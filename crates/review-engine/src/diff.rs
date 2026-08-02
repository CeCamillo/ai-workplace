//! The changeset an Agent Worktree holds, as the engine models it: files,
//! hunks, and diff lines — the input the Review Pane's stream is built from.

/// Everything the agent changed in its Agent Worktree, ordered as git
/// reports it.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Changeset {
    pub files: Vec<FileDiff>,
}

/// One changed file and its hunks. `path` is the post-change path,
/// repo-relative.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileDiff {
    pub path: String,
    pub hunks: Vec<Hunk>,
}

/// One contiguous run of changes, as delimited by a `@@` header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk {
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiffLine {
    pub kind: DiffLineKind,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Context,
    Added,
    Removed,
}

/// Parse `git diff` unified output into a [`Changeset`]. Unknown lines
/// (mode changes, index lines, binary notices) are skipped; a file with no
/// hunks (e.g. binary) still appears so it can be marked reviewed.
pub fn parse_unified_diff(text: &str) -> Changeset {
    let mut files: Vec<FileDiff> = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            files.push(FileDiff {
                path: path_from_diff_header(rest),
                hunks: Vec::new(),
            });
        } else if let Some(path) = line.strip_prefix("+++ b/") {
            // The post-image path is authoritative when present (renames).
            if let Some(file) = files.last_mut() {
                path.clone_into(&mut file.path);
            }
        } else if line.starts_with("@@") {
            if let Some(file) = files.last_mut() {
                file.hunks.push(Hunk {
                    header: line.to_string(),
                    lines: Vec::new(),
                });
            }
        } else if let Some(hunk) = files.last_mut().and_then(|f| f.hunks.last_mut()) {
            let (kind, content) = match line.split_at_checked(1) {
                Some(("+", rest)) => (DiffLineKind::Added, rest),
                Some(("-", rest)) => (DiffLineKind::Removed, rest),
                Some((" ", rest)) => (DiffLineKind::Context, rest),
                // "\ No newline at end of file" and anything else.
                _ => continue,
            };
            hunk.lines.push(DiffLine {
                kind,
                content: content.to_string(),
            });
        }
    }
    Changeset { files }
}

/// `a/old b/new` → `new`. Quoted paths (spaces, unicode) keep their quoted
/// form; good enough for display until a real need appears.
fn path_from_diff_header(rest: &str) -> String {
    rest.rfind(" b/")
        .map(|idx| rest[idx + 3..].to_string())
        .unwrap_or_else(|| rest.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 111..222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,3 +1,4 @@
 mod a;
+mod b;
@@ -10,2 +11,2 @@
-old()
+new()
diff --git a/src/new.rs b/src/new.rs
new file mode 100644
index 000..333
--- /dev/null
+++ b/src/new.rs
@@ -0,0 +1,2 @@
+pub fn b() {}
+// done
\\ No newline at end of file
";

    #[test]
    fn parses_files_hunks_and_line_kinds() {
        let changeset = parse_unified_diff(SAMPLE);

        assert_eq!(changeset.files.len(), 2);
        let lib = &changeset.files[0];
        assert_eq!(lib.path, "src/lib.rs");
        assert_eq!(lib.hunks.len(), 2);
        assert_eq!(lib.hunks[0].header, "@@ -1,3 +1,4 @@");
        assert_eq!(
            lib.hunks[0].lines,
            vec![
                DiffLine {
                    kind: DiffLineKind::Context,
                    content: "mod a;".to_string()
                },
                DiffLine {
                    kind: DiffLineKind::Added,
                    content: "mod b;".to_string()
                },
            ]
        );
        assert_eq!(lib.hunks[1].lines[0].kind, DiffLineKind::Removed);

        let new = &changeset.files[1];
        assert_eq!(new.path, "src/new.rs");
        assert_eq!(new.hunks.len(), 1);
        assert_eq!(new.hunks[0].lines.len(), 2, "no-newline marker is skipped");
    }

    #[test]
    fn a_binary_file_still_appears_with_no_hunks() {
        let diff = "\
diff --git a/logo.png b/logo.png
index 111..222 100644
Binary files a/logo.png and b/logo.png differ
";
        let changeset = parse_unified_diff(diff);
        assert_eq!(changeset.files.len(), 1);
        assert_eq!(changeset.files[0].path, "logo.png");
        assert!(changeset.files[0].hunks.is_empty());
    }

    #[test]
    fn an_empty_diff_is_an_empty_changeset() {
        assert_eq!(parse_unified_diff(""), Changeset::default());
    }
}
