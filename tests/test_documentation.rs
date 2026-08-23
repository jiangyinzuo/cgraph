use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

const TESTING_DOCUMENTATION: &str = include_str!("../src/testing/README.md");

#[test]
fn documented_test_inventory_matches_source_annotations() {
    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let (expected, documented_total) = documented_inventory(TESTING_DOCUMENTATION);
    let actual = source_inventory(&workspace);
    let actual_total = actual.values().sum::<usize>();

    assert_eq!(
        actual, expected,
        "update src/testing/README.md when tests change"
    );
    assert_eq!(
        actual_total, documented_total,
        "documented test total must equal the per-file inventory"
    );
}

fn documented_inventory(documentation: &str) -> (BTreeMap<String, usize>, usize) {
    let block = documentation
        .split_once("<!-- test-inventory\n")
        .and_then(|(_, remainder)| remainder.split_once("\n-->"))
        .map(|(block, _)| block)
        .expect("src/testing/README.md must contain a test-inventory block");
    let mut inventory = BTreeMap::new();
    let mut total = None;
    for line in block.lines() {
        let (path, count) = line
            .split_once(':')
            .expect("test-inventory lines must use path: count");
        let count = count
            .trim()
            .parse::<usize>()
            .expect("test-inventory counts must be integers");
        if path == "total" {
            total = Some(count);
        } else {
            inventory.insert(path.to_owned(), count);
        }
    }
    let total = total.expect("test-inventory must contain total");
    assert_eq!(
        inventory.values().sum::<usize>(),
        total,
        "documented per-file test counts must add up to total"
    );
    (inventory, total)
}

fn source_inventory(workspace: &Path) -> BTreeMap<String, usize> {
    let mut inventory = BTreeMap::new();
    for directory in ["src", "tests", "examples"] {
        collect_rust_tests(workspace, &workspace.join(directory), &mut inventory);
    }
    inventory
}

fn collect_rust_tests(workspace: &Path, directory: &Path, inventory: &mut BTreeMap<String, usize>) {
    let entries = fs::read_dir(directory)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", directory.display()));
    for entry in entries {
        let path = entry.expect("failed to read directory entry").path();
        if path.is_dir() {
            collect_rust_tests(workspace, &path, inventory);
            continue;
        }
        if path.extension().and_then(|extension| extension.to_str()) != Some("rs") {
            continue;
        }
        let source = fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()));
        let count = source
            .lines()
            .filter(|line| {
                let line = line.trim();
                line == "#[test]" || line.starts_with("#[tokio::test")
            })
            .count();
        if count == 0 {
            continue;
        }
        let relative = path
            .strip_prefix(workspace)
            .expect("test source must be inside the workspace")
            .to_string_lossy()
            .replace('\\', "/");
        inventory.insert(relative, count);
    }
}
