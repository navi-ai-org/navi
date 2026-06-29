mod catalog;
mod slug;

pub use catalog::{find_product_by_slug, Product};
pub use slug::slugify;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugify_trims_punctuation_and_collapses_separators() {
        assert_eq!(
            slugify("  NAVI Tutor: Visual Study!  "),
            "navi-tutor-visual-study"
        );
        assert_eq!(slugify("Rust---Tools"), "rust-tools");
        assert_eq!(slugify("Local_agent runtime"), "local-agent-runtime");
    }

    #[test]
    fn product_lookup_uses_canonical_slug() {
        let products = [
            Product::new(1, "NAVI Tutor: Visual Study"),
            Product::new(2, "Local Agent Runtime"),
        ];

        let product = find_product_by_slug(&products, "navi-tutor-visual-study")
            .expect("product should be found by canonical slug");

        assert_eq!(product.id, 1);
    }
}
