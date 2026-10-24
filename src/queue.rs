//! The job queue: owns job state and runs jobs on worker threads.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::jobs::{self, JobError, JobSpec};

pub type JobId = u64;

/// Called from worker threads whenever there is news (e.g. request a repaint).
pub type Notifier = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone, Debug, PartialEq)]
pub enum JobState {
    Queued,
    Running { progress: Option<f32> },
    Done { elapsed: Duration },
    Failed(String),
    Cancelled,
}

impl JobState {
    pub fn is_finished(&self) -> bool {
        matches!(self, Self::Done { .. } | Self::Failed(_) | Self::Cancelled)
    }
}

pub struct Job {
    pub id: JobId,
    pub spec: JobSpec,
    pub state: JobState,
    cancel: Arc<AtomicBool>,
    started: Option<Instant>,
}

enum Event {
    Progress(JobId, f32),
    Finished(JobId, Result<(), JobError>),
}

pub struct Queue {
    jobs: Vec<Job>,
    next_id: JobId,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    workers: Vec<JoinHandle<()>>,
    /// When false, queued jobs wait; running ones continue.
    pub is_running: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub queued: usize,
    pub running: usize,
    pub done: usize,
    pub failed: usize,
}

impl Queue {
    pub fn new(is_running: bool) -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            jobs: Vec::new(),
            next_id: 1,
            tx,
            rx,
            workers: Vec::new(),
            is_running,
        }
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn push(&mut self, spec: JobSpec) {
        self.jobs.push(Job {
            id: self.next_id,
            spec,
            state: JobState::Queued,
            cancel: Arc::new(AtomicBool::new(false)),
            started: None,
        });
        self.next_id += 1;
    }

    /// Output paths of jobs that haven't failed, so new jobs don't collide with them.
    pub fn claimed_outputs(&self) -> impl Iterator<Item = &PathBuf> {
        self.jobs
            .iter()
            .filter(|j| !matches!(j.state, JobState::Failed(_) | JobState::Cancelled))
            .map(|j| &j.spec.output)
    }

    pub fn counts(&self) -> Counts {
        self.jobs.iter().fold(Counts::default(), |mut c, job| {
            match job.state {
                JobState::Queued => c.queued += 1,
                JobState::Running { .. } => c.running += 1,
                JobState::Done { .. } => c.done += 1,
                JobState::Failed(_) | JobState::Cancelled => c.failed += 1,
            }
            c
        })
    }

    /// Cancels a job: queued ones immediately, running ones once ffmpeg is stopped.
    pub fn cancel(&mut self, id: JobId) {
        if let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) {
            match job.state {
                JobState::Queued => job.state = JobState::Cancelled,
                JobState::Running { .. } => job.cancel.store(true, Ordering::Relaxed),
                _ => {}
            }
        }
    }

    pub fn cancel_all(&mut self) {
        self.is_running = false;
        let ids: Vec<JobId> = self.jobs.iter().map(|j| j.id).collect();
        ids.into_iter().for_each(|id| self.cancel(id));
    }

    /// Removes a job that isn't running.
    pub fn remove(&mut self, id: JobId) {
        self.jobs
            .retain(|j| j.id != id || matches!(j.state, JobState::Running { .. }));
    }

    pub fn clear_finished(&mut self) {
        self.jobs.retain(|j| !j.state.is_finished());
    }

    /// Re-queues failed or cancelled jobs.
    pub fn retry_failed(&mut self) {
        for job in &mut self.jobs {
            if matches!(job.state, JobState::Failed(_) | JobState::Cancelled) {
                job.state = JobState::Queued;
                job.cancel.store(false, Ordering::Relaxed);
            }
        }
    }

    /// Applies worker events and starts queued jobs up to `max_parallel`.
    /// Call once per frame; cheap when nothing changed.
    pub fn pump(&mut self, max_parallel: usize, ffmpeg: Option<&PathBuf>, notify: &Notifier) {
        while let Ok(event) = self.rx.try_recv() {
            self.apply(event);
        }
        self.workers.retain(|w| !w.is_finished());
        if !self.is_running {
            return;
        }
        let running = self.counts().running;
        let free = max_parallel.max(1).saturating_sub(running);
        let ids: Vec<JobId> = self
            .jobs
            .iter()
            .filter(|j| j.state == JobState::Queued)
            .take(free)
            .map(|j| j.id)
            .collect();
        for id in ids {
            self.start(id, ffmpeg.cloned(), notify.clone());
        }
    }

    fn apply(&mut self, event: Event) {
        let id = match &event {
            Event::Progress(id, _) | Event::Finished(id, _) => *id,
        };
        let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) else {
            return;
        };
        job.state = match event {
            // A late progress tick must not resurrect a job that already finished.
            Event::Progress(_, _) if job.state.is_finished() => return,
            Event::Progress(_, p) => JobState::Running { progress: Some(p) },
            Event::Finished(_, Ok(())) => JobState::Done {
                elapsed: job.started.map(|s| s.elapsed()).unwrap_or_default(),
            },
            Event::Finished(_, Err(e)) if e.is_cancelled() => JobState::Cancelled,
            Event::Finished(_, Err(e)) => {
                tracing::warn!(job = id, error = %e, "job failed");
                JobState::Failed(e.to_string())
            }
        };
    }

    fn start(&mut self, id: JobId, ffmpeg: Option<PathBuf>, notify: Notifier) {
        let Some(job) = self.jobs.iter_mut().find(|j| j.id == id) else {
            return;
        };
        job.state = JobState::Running { progress: None };
        job.started = Some(Instant::now());
        let spec = job.spec.clone();
        let cancel = Arc::clone(&job.cancel);
        let tx = self.tx.clone();
        let handle = thread::Builder::new()
            .name(format!("job-{id}"))
            .spawn(move || {
                let progress_tx = tx.clone();
                let progress_notify = Arc::clone(&notify);
                let result = jobs::execute(&spec, ffmpeg.as_deref(), &cancel, move |p| {
                    let _ = progress_tx.send(Event::Progress(id, p));
                    progress_notify();
                });
                let _ = tx.send(Event::Finished(id, result));
                notify();
            });
        match handle {
            Ok(handle) => self.workers.push(handle),
            Err(e) => job.state = JobState::Failed(JobError::Io(e).to_string()),
        }
    }
}

impl Drop for Queue {
    /// Stops every ffmpeg child before the app exits so none are orphaned.
    fn drop(&mut self) {
        self.cancel_all();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{ImageFormat, Tool};
    use crate::options::Options;

    fn spec(dir: &std::path::Path, name: &str) -> JobSpec {
        let input = dir.join(format!("{name}.png"));
        image::RgbImage::new(8, 8).save(&input).expect("sample");
        JobSpec {
            tool: Tool::ToImage(ImageFormat::Bmp),
            inputs: vec![input],
            output: dir.join(format!("{name}.bmp")),
            options: Options::default(),
        }
    }

    fn run_until_idle(queue: &mut Queue, notify: &Notifier) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while queue.counts().queued + queue.counts().running > 0 {
            assert!(Instant::now() < deadline, "queue did not finish");
            queue.pump(2, None, notify);
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn runs_jobs_to_completion_in_parallel_limit() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut queue = Queue::new(false);
        for name in ["a", "b", "c"] {
            queue.push(spec(dir.path(), name));
        }
        let notify: Notifier = Arc::new(|| {});
        queue.pump(2, None, &notify);
        assert_eq!(queue.counts().queued, 3, "paused queue must not start");
        queue.is_running = true;
        run_until_idle(&mut queue, &notify);
        assert_eq!(queue.counts().done, 3);
        assert!(dir.path().join("b.bmp").is_file());
    }

    #[test]
    fn missing_ffmpeg_fails_media_jobs_with_message() {
        let mut queue = Queue::new(false);
        queue.push(JobSpec {
            tool: Tool::Trim,
            inputs: vec![PathBuf::from("/nope.mp4")],
            output: PathBuf::from("/nope_trim.mp4"),
            options: Options::default(),
        });
        queue.is_running = true;
        let notify: Notifier = Arc::new(|| {});
        run_until_idle(&mut queue, &notify);
        assert!(matches!(queue.jobs()[0].state, JobState::Failed(_)));
        queue.retry_failed();
        assert_eq!(queue.counts().queued, 1);
    }

    #[test]
    fn cancelling_queued_job_and_clearing() {
        let mut queue = Queue::new(false);
        let dir = tempfile::tempdir().expect("tempdir");
        queue.push(spec(dir.path(), "x"));
        queue.cancel(1);
        assert_eq!(queue.jobs()[0].state, JobState::Cancelled);
        queue.clear_finished();
        assert!(queue.jobs().is_empty());
    }
}
