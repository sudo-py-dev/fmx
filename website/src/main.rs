use std::{fs, io, path::Path};
fn copy_tree(from: &Path, to: &Path) -> io::Result<()> {
    fs::create_dir_all(to)?;
    for entry in fs::read_dir(from)? {
        let entry = entry?;
        let target = to.join(entry.file_name());
        if entry.file_type()?.is_dir() { copy_tree(&entry.path(), &target)?; }
        else { fs::copy(entry.path(), target)?; }
    }
    Ok(())
}
fn main() -> io::Result<()> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    copy_tree(&root.join("static"), &root.join("dist"))?;
    println!("Built FMX download website in {}", root.join("dist").display());
    Ok(())
}
