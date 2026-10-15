//! A line diff between two versions: the longest common subsequence of lines, with old and new line numbers.

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Same,
    Add,
    Del,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DiffLine {
    pub op: Op,
    /// 1-based line in the old text (`None` for an added line).
    pub old: Option<u32>,
    /// 1-based line in the new text (`None` for a removed line).
    pub new: Option<u32>,
    pub text: String,
}

/// The lines of `old` and `new` in order, each marked same, added or removed. Common leading and trailing lines are
/// trimmed before the O(n·m) table, so an edit in a long prompt costs the size of the edited region.
pub fn diff(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = old.lines().collect();
    let b: Vec<&str> = new.lines().collect();
    let prefix = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..]
        .iter()
        .rev()
        .zip(b[prefix..].iter().rev())
        .take_while(|(x, y)| x == y)
        .count();
    let (am, bm) = (&a[prefix..a.len() - suffix], &b[prefix..b.len() - suffix]);

    // lcs[i][j]: the LCS length of am[i..] and bm[j..].
    let (n, m) = (am.len(), bm.len());
    let mut lcs = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if am[i] == bm[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }

    let mut out = Vec::with_capacity(a.len().max(b.len()));
    let same = |out: &mut Vec<DiffLine>, i: usize, j: usize, text: &str| {
        out.push(DiffLine {
            op: Op::Same,
            old: Some(i as u32 + 1),
            new: Some(j as u32 + 1),
            text: text.to_string(),
        });
    };
    for (k, line) in a[..prefix].iter().enumerate() {
        same(&mut out, k, k, line);
    }
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && am[i] == bm[j] {
            same(&mut out, prefix + i, prefix + j, am[i]);
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            out.push(DiffLine {
                op: Op::Add,
                old: None,
                new: Some((prefix + j) as u32 + 1),
                text: bm[j].to_string(),
            });
            j += 1;
        } else {
            out.push(DiffLine {
                op: Op::Del,
                old: Some((prefix + i) as u32 + 1),
                new: None,
                text: am[i].to_string(),
            });
            i += 1;
        }
    }
    for k in 0..suffix {
        let (oi, ni) = (a.len() - suffix + k, b.len() - suffix + k);
        same(&mut out, oi, ni, a[oi]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ops(d: &[DiffLine]) -> String {
        d.iter()
            .map(|l| match l.op {
                Op::Same => '=',
                Op::Add => '+',
                Op::Del => '-',
            })
            .collect()
    }

    #[test]
    fn diff_marks_lines_as_same_add_or_del() {
        // equal_texts_are_all_same: identical texts give only Same lines, numbered on both sides
        {
            let d = diff("a\nb\n", "a\nb\n");
            assert_eq!(ops(&d), "==");
            assert_eq!((d[1].old, d[1].new), (Some(2), Some(2)));
        }
        // an_edit_in_the_middle: an added and a removed line carry the right numbers
        {
            let d = diff("a\nb\nc\nd", "a\nB\nc\nd\ne");
            assert_eq!(ops(&d), "=+-==+");
            assert_eq!(
                d[1],
                DiffLine {
                    op: Op::Add,
                    old: None,
                    new: Some(2),
                    text: "B".into()
                }
            );
            assert_eq!(
                d[2],
                DiffLine {
                    op: Op::Del,
                    old: Some(2),
                    new: None,
                    text: "b".into()
                }
            );
            assert_eq!((d[4].old, d[4].new), (Some(4), Some(4)));
            assert_eq!(d[5].new, Some(5));
        }
        // from_and_to_empty: an empty side gives only Add or only Del lines
        {
            assert_eq!(ops(&diff("", "x\ny")), "++");
            assert_eq!(ops(&diff("x\ny", "")), "--");
        }
    }
}
