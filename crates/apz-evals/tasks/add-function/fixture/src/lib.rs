pub fn sub(a: i32, b: i32) -> i32 {
    a - b
}

#[cfg(test)]
mod tests {
    use super::sub;

    #[test]
    fn sub_subtracts() {
        assert_eq!(sub(5, 3), 2);
    }
}
