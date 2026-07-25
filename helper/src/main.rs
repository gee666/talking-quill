use std::path::Path;

fn main() {
    let arguments: Vec<String> = std::env::args().collect();
    let result = if arguments.get(1).map(String::as_str) == Some("--remove-owned-tree") {
        match (arguments.get(2), arguments.get(3), arguments.len()) {
            (Some(path), Some(identity), 4) => {
                talking_quill_helper::owned_tree::remove_owned_tree(Path::new(path), identity)
                    .map_err(|error| error.to_string())
            }
            _ => Err("expected --remove-owned-tree <path> <device:inode>".to_owned()),
        }
    } else if arguments.len() == 1 {
        talking_quill_helper::run().map_err(|error| error.to_string())
    } else {
        Err("unknown talking-quill-helper arguments".to_owned())
    };
    if let Err(error) = result {
        eprintln!("talking-quill-helper: {error}");
        std::process::exit(1);
    }
}
