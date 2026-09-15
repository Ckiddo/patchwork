#[path = "../tests/support/mod.rs"]
mod support;

fn main() {
    print!("{}", support::schema_manifest());
}
