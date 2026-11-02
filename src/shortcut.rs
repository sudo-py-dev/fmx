//! Per-tool desktop launchers. IDs use the same stable representation as settings.
use crate::catalog::Tool;
use std::{
    fs, io,
    path::{Path, PathBuf},
};

fn tool_id(tool: Tool) -> String {
    serde_json::to_vec(&tool)
        .expect("tool serializes")
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

pub fn parse_tool(id: &str) -> Option<Tool> {
    if id.len() > 512 || !id.len().is_multiple_of(2) || !id.is_ascii() {
        return None;
    }
    let bytes: Option<Vec<u8>> = (0..id.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&id[i..i + 2], 16).ok())
        .collect();
    serde_json::from_slice(&bytes?).ok()
}

pub fn create(tool: Tool) -> io::Result<PathBuf> {
    let desktop =
        dirs::desktop_dir().ok_or_else(|| io::Error::other("Desktop folder unavailable"))?;
    // Do not accidentally create launchers in home when desktop icons are disabled.
    if Some(desktop.as_path()) == dirs::home_dir().as_deref() {
        return Err(io::Error::other("Desktop folder unavailable"));
    }
    fs::create_dir_all(&desktop)?;
    let exe = std::env::var_os("APPIMAGE")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_exe()?);
    let id = tool_id(tool);
    let name = format!(
        "FMX — {} ({})",
        tool.label().replace(['\n', '\r', '\u{200f}'], ""),
        tool.category()
            .label()
            .replace(['\n', '\r', '\u{200f}'], "")
    );
    write_launcher(&desktop, &exe, &id, &name)
}

#[cfg(target_os = "linux")]
fn desktop_arg(value: &str) -> String {
    // Exec quoting is followed by desktop-entry string escaping.
    let value = value
        .replace('%', "%%")
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$");
    format!("\"{value}\"").replace('\\', "\\\\")
}

#[cfg(target_os = "linux")]
fn write_launcher(desktop: &Path, exe: &Path, id: &str, name: &str) -> io::Result<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let path = desktop.join(format!("fmx-tool-{id}.desktop"));
    let exe = exe
        .to_str()
        .filter(|s| !s.contains(['\n', '\r']))
        .ok_or_else(|| io::Error::other("Unsupported executable path"))?;
    let name = name.replace('\\', "\\\\");
    let contents = format!(
        "[Desktop Entry]\nType=Application\nName={name}\nExec={} --tool {id}\nIcon=fmx\nTerminal=false\n",
        desktop_arg(exe)
    );
    fs::write(&path, contents)?;
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
    // GNOME may still ask the user to allow launching; this metadata is best effort.
    let _ = crate::process::command(Path::new("gio"))
        .args(["set", "-t", "string"])
        .arg(&path)
        .args(["metadata::trusted", "true"])
        .output();
    Ok(path)
}

#[cfg(windows)]
fn write_launcher(desktop: &Path, exe: &Path, id: &str, name: &str) -> io::Result<PathBuf> {
    let safe_name: String = name
        .chars()
        .map(|c| {
            if "<>:\"/\\|?*".contains(c) || c.is_control() {
                '_'
            } else {
                c
            }
        })
        .collect();
    let path = desktop.join(format!("{safe_name}.lnk"));
    let quote = |s: &std::ffi::OsStr| format!("'{}'", s.to_string_lossy().replace('\'', "''"));
    let script = format!(
        "$ErrorActionPreference='Stop'; $s=(New-Object -ComObject WScript.Shell).CreateShortcut({}); $s.TargetPath={}; $s.Arguments='--tool {id}'; $s.Description={}; $s.Save()",
        quote(path.as_os_str()),
        quote(exe.as_os_str()),
        quote(std::ffi::OsStr::new(name))
    );
    let output = crate::process::command(Path::new("powershell.exe"))
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()?;
    if !output.status.success() {
        return Err(io::Error::other(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    Ok(path)
}

#[cfg(not(any(windows, target_os = "linux")))]
fn write_launcher(_desktop: &Path, _exe: &Path, _id: &str, _name: &str) -> io::Result<PathBuf> {
    Err(io::Error::other(
        "Desktop shortcuts are unavailable on this platform",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_tool_round_trips() {
        for tool in crate::catalog::Category::ALL
            .into_iter()
            .flat_map(crate::catalog::Category::tools)
        {
            assert_eq!(parse_tool(&tool_id(tool)), Some(tool));
        }
        for id in ["", "x", "zz", "éé", "00"] {
            assert_eq!(parse_tool(id), None);
        }
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn writes_a_tool_launcher_and_reuses_its_path() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let id = tool_id(Tool::RecordAudio);
        let exe = Path::new("/opt/my app/fmx");
        let path = write_launcher(dir.path(), exe, &id, "FMX — Record audio").unwrap();
        let contents = fs::read_to_string(&path).unwrap();
        assert!(contents.contains(&format!("Exec=\"/opt/my app/fmx\" --tool {id}\n")));
        assert!(contents.contains("Terminal=false\n"));
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o755
        );
        assert_eq!(
            write_launcher(dir.path(), exe, &id, "FMX — Record audio").unwrap(),
            path
        );
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn exec_escapes_special_characters() {
        assert_eq!(
            desktop_arg("/opt/my app/100%fmx"),
            "\"/opt/my app/100%%fmx\""
        );
        assert_eq!(desktop_arg("a\"$`\\b"), "\"a\\\\\"\\\\$\\\\`\\\\\\\\b\"");
    }
}
