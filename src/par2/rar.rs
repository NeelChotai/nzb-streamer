

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum RarExt {
    Main,
    Part(u32),
}

impl RarExt {
    pub fn from_filename(filename: &str) -> Option<Self> {
        if filename.ends_with(".rar") {
            Some(RarExt::Main)
        } else {
            extract_rar_number(filename).map(RarExt::Part)
        }
    }
}

impl Ord for RarExt {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        use RarExt::*;
        match (self, other) {
            (Main, Main) => std::cmp::Ordering::Equal,
            (Main, _) => std::cmp::Ordering::Less,
            (_, Main) => std::cmp::Ordering::Greater,
            (Part(a), Part(b)) => a.cmp(b),
        }
    }
}

impl PartialOrd for RarExt {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

fn extract_rar_number(filename: &str) -> Option<u32> {
    filename
        .rsplit('.')
        .next()
        .filter(|ext| ext.starts_with('r') && ext.len() > 1)
        .and_then(|ext| ext[1..].parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_rar_number() {
        let cases = [
            ("file.rar", None),
            ("file.r00", Some(0)),
            ("file.r01", Some(1)),
            ("file.r99", Some(99)),
            ("file.r9999", Some(9999)),
            ("roll.roll.on.r00", Some(0)),
            ("path/to/file.r05", Some(5)),
            ("file.ra0", None), // Not a number
            ("file.r", None),   // No digits
            ("r00", None),      // No dot
            ("file.txt", None), // Wrong extension
        ];

        for (input, expected) in cases {
            assert_eq!(
                extract_rar_number(input),
                expected,
                "Failed for input: {input}"
            );
        }
    }

    #[test]
    fn test_rar_sorting() {
        let mut files = vec!["file.r02", "file.r00", "file.rar", "file.r10", "file.r01"];

        files.sort_by_key(|name| {
            if name.ends_with(".rar") {
                (0, 0)
            } else {
                (1, extract_rar_number(name).unwrap_or(999))
            }
        });

        assert_eq!(
            files,
            vec!["file.rar", "file.r00", "file.r01", "file.r02", "file.r10",]
        );
    }
}
