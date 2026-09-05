use super::*;
use std::{fs, path::PathBuf};

fn identity(path: &Path) -> String {
    platform::identity(path).unwrap()
}

fn temporary(name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "talking-quill-owned-tree-{name}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ))
}

#[test]
fn rejects_a_directory_replacement_and_preserves_unrelated_data() {
    let parent = temporary("replacement");
    let root = parent.join("owned");
    let moved = parent.join("moved-owned");
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::write(root.join("nested/private"), b"private").unwrap();
    let expected = identity(&root);
    fs::rename(&root, &moved).unwrap();
    fs::create_dir(&root).unwrap();
    fs::write(root.join("unrelated"), b"preserve").unwrap();

    assert!(matches!(
        remove_owned_tree(&root, &expected),
        Err(OwnedTreeError::IdentityMismatch)
    ));
    assert_eq!(fs::read(root.join("unrelated")).unwrap(), b"preserve");
    assert_eq!(fs::read(moved.join("nested/private")).unwrap(), b"private");
    fs::remove_dir_all(parent).unwrap();
}

#[test]
fn removes_only_the_identity_bound_tree() {
    let parent = temporary("remove");
    let root = parent.join("owned");
    let outside = parent.join("outside");
    fs::create_dir_all(root.join("nested")).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(root.join("nested/private"), b"private").unwrap();
    fs::write(outside.join("sentinel"), b"preserve").unwrap();
    let expected = identity(&root);

    remove_owned_tree(&root, &expected).unwrap();
    assert!(!root.exists());
    assert_eq!(fs::read(outside.join("sentinel")).unwrap(), b"preserve");
    fs::remove_dir_all(parent).unwrap();
}
