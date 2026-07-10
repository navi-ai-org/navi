//! Tiny fixture for the full-tool + cache-hit audit.
//! Symbols exist so ast_search / symbol_goto / code tools have targets.

pub fn greet(name: &str) -> String {
    format!("hello, {name}")
}

pub fn add(a: i32, b: i32) -> i32 {
    a + b
}

fn main() {
    println!("{}", greet("navi"));
    println!("sum={}", add(2, 3));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greet_works() {
        assert!(greet("x").contains("x"));
    }

    #[test]
    fn add_works() {
        assert_eq!(add(1, 2), 3);
    }
}
