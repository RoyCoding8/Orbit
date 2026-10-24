#[cfg(windows)]
use std::path::PathBuf;

pub(crate) fn tempdir() -> std::io::Result<tempfile::TempDir> {
    let root = std::env::temp_dir().canonicalize()?;
    #[cfg(windows)]
    let root = strip_verbatim_prefix(root);
    tempfile::tempdir_in(root)
}

#[cfg(windows)]
fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    use std::path::{Component, Prefix};

    let mut components = path.components();
    let Some(Component::Prefix(prefix)) = components.next() else {
        return path;
    };
    let mut plain = match prefix.kind() {
        Prefix::VerbatimDisk(drive) => PathBuf::from(format!("{}:\\", char::from(drive))),
        Prefix::VerbatimUNC(server, share) => PathBuf::from(r"\\").join(server).join(share),
        _ => return path,
    };
    for component in components.skip(1) {
        plain.push(component.as_os_str());
    }
    plain
}
