pub fn compute_total(items: &[u32]) -> u32 {
    items.iter().sum()
}

#[cfg(test)]
mod tests {
    use super::compute_total;

    #[test]
    fn totals_the_items() {
        assert_eq!(compute_total(&[1, 2, 3]), 6);
    }
}
