//! The Windows app's windows, which only exist on Windows (macOS has the Swift app's): see imp.rs.

#[cfg(windows)]
mod imp;

fn main() {
    #[cfg(windows)]
    imp::main();
    #[cfg(not(windows))]
    eprintln!("snapshot works with the Windows app's windows; it runs on Windows");
}
