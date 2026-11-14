// Embedded Python: tell the linker where libpython / Python3.framework lives at run time.
// Uses the same interpreter PyO3 builds against (PYO3_PYTHON, else python3).
fn main() {
    println!("cargo:rerun-if-env-changed=PYO3_PYTHON");
    let python = std::env::var("PYO3_PYTHON").unwrap_or_else(|_| "python3".into());
    let out = std::process::Command::new(&python)
        .args(["-c", "import sysconfig as s; print(s.get_config_var('PYTHONFRAMEWORKPREFIX') or s.get_config_var('LIBDIR'))"])
        .output()
        .expect("could not run Python to find its library");
    let dir = String::from_utf8(out.stdout).unwrap();
    println!("cargo:rustc-link-arg=-Wl,-rpath,{}", dir.trim());
}
