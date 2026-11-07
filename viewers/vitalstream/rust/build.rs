// The page in <repo>/web/ is embedded by include_dir!; rebuild when any of it changes.
fn main() {
    println!("cargo:rerun-if-changed=../web");
}
