use crate::slugify;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Product {
    pub id: u32,
    pub name: String,
}

impl Product {
    pub fn new(id: u32, name: impl Into<String>) -> Self {
        Self {
            id,
            name: name.into(),
        }
    }
}

pub fn find_product_by_slug<'a>(products: &'a [Product], slug: &str) -> Option<&'a Product> {
    products
        .iter()
        .find(|product| product.name.to_ascii_lowercase().replace(' ', "-") == slug)
}
