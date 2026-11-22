//! Types and pure logic shared by the server and the browser (WASM).

use serde::{Deserialize, Serialize};

/// A product of the catalog.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Product {
    pub id: i64,
    pub name: String,
    /// Position in the shop: products are listed in this order while shopping.
    pub category: i64,
    /// How many times the product has been put on the list.
    pub count: i64,
}

/// A product recently taken off the shopping list.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Removed {
    pub product: Product,
    /// When it was taken, already formatted ("oggi 10:32"); empty if unknown.
    pub when: String,
}

/// Everything the UI needs, loaded in a single request.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// Products *not* on the list, most frequently bought first.
    pub catalog: Vec<Product>,
    /// The shopping list, ordered by category.
    pub list: Vec<Product>,
    /// Products taken most recently, newest first.
    pub removed: Vec<Removed>,
}

/// Reference categories (value = position in the shop) offered by autocompletion.
pub const CATEGORIES: &[(i64, &str)] = &[
    (0, "Varie"),
    (10, "Scuola"),
    (15, "Vestiti"),
    (20, "Pentole"),
    (25, "Cura della Persona"),
    (30, "Detersivi"),
    (35, "Pane"),
    (40, "Colazione"),
    (45, "Pasta"),
    (50, "Latte"),
    (55, "Succhi"),
    (60, "Verdure"),
    (65, "Freddi"),
    (70, "Carni"),
    (75, "Surgelati"),
];

/// Text color of each group of 5 categories on the shopping list.
const GROUP_COLORS: [&str; 21] = [
    "rgb(0, 0, 0)",
    "rgb(128, 0, 0)",
    "rgb(255, 0, 0)",
    "rgb(0, 128, 0)",
    "rgb(0, 200, 0)",
    "rgb(0, 0, 128)",
    "rgb(0, 0, 255)",
    "rgb(128, 128, 0)",
    "rgb(128, 0, 128)",
    "rgb(0, 128, 128)",
    "rgb(255, 128, 128)",
    "rgb(255, 128, 0)",
    "rgb(0, 128, 255)",
    "rgb(64, 128, 170)",
    "rgb(170, 128, 64)",
    "rgb(0, 64, 128)",
    "rgb(64, 200, 128)",
    "rgb(128, 200, 64)",
    "rgb(64, 64, 128)",
    "rgb(64, 128, 64)",
    "rgb(128, 64, 64)",
];

/// Categories are grouped 5 by 5 (0-4, 5-9, ...).
pub fn group_of(category: i64) -> i64 {
    category / 5
}

/// Color used for a product of this category on the shopping list.
pub fn category_color(category: i64) -> &'static str {
    let last = GROUP_COLORS.len() as i64 - 1;
    GROUP_COLORS[group_of(category).clamp(0, last) as usize]
}

/// Name of the reference category this value falls into (e.g. 66 -> "Freddi").
pub fn category_label(category: i64) -> &'static str {
    CATEGORIES
        .iter()
        .rev()
        .find(|(value, _)| *value <= category)
        .map_or(CATEGORIES[0].1, |(_, label)| label)
}

/// Parses a category typed by the user.
pub fn parse_category(s: &str) -> Option<i64> {
    s.trim().parse().ok()
}

fn contains_ci(haystack: &str, needle: &str) -> bool {
    haystack.to_lowercase().contains(&needle.to_lowercase())
}

/// Products whose name contains `query` (case-insensitive), at most `limit`.
pub fn suggest_products<'a>(catalog: &'a [Product], query: &str, limit: usize) -> Vec<&'a Product> {
    let query = query.trim();
    if query.is_empty() {
        return Vec::new();
    }
    catalog
        .iter()
        .filter(|p| contains_ci(&p.name, query))
        .take(limit)
        .collect()
}

/// Reference categories matching `query` by name or by number.
pub fn suggest_categories(query: &str) -> Vec<(i64, &'static str)> {
    let query = query.trim();
    CATEGORIES
        .iter()
        .filter(|(value, label)| {
            query.is_empty() || contains_ci(label, query) || value.to_string().starts_with(query)
        })
        .copied()
        .collect()
}

/// Pairs each product with a flag telling whether it starts a new category group.
pub fn with_group_breaks(list: Vec<Product>) -> Vec<(Product, bool)> {
    let mut previous = None;
    list.into_iter()
        .map(|p| {
            let group = group_of(p.category);
            let brk = previous.is_some_and(|g| g != group);
            previous = Some(group);
            (p, brk)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn product(id: i64, name: &str, category: i64) -> Product {
        Product { id, name: name.into(), category, count: 0 }
    }

    #[test]
    fn colors_and_labels() {
        assert_eq!(category_color(66), "rgb(64, 128, 170)"); // group 13
        assert_eq!(category_color(-3), "rgb(0, 0, 0)");
        assert_eq!(category_color(500), "rgb(128, 64, 64)");
        assert_eq!(category_label(66), "Freddi");
        assert_eq!(category_label(3), "Varie");
    }

    #[test]
    fn suggestions() {
        let catalog = vec![product(1, "Mozzarella", 66), product(2, "Pane", 35)];
        let found: Vec<_> = suggest_products(&catalog, "MOZZ", 10).iter().map(|p| p.id).collect();
        assert_eq!(found, vec![1]);
        assert!(suggest_products(&catalog, "  ", 10).is_empty());
        assert_eq!(suggest_categories("fre"), vec![(65, "Freddi")]);
        assert_eq!(suggest_categories("7"), vec![(70, "Carni"), (75, "Surgelati")]);
        assert_eq!(suggest_categories("").len(), CATEGORIES.len());
    }

    #[test]
    fn group_breaks() {
        let list = vec![product(1, "a", 66), product(2, "b", 67), product(3, "c", 70)];
        let flags: Vec<_> = with_group_breaks(list).into_iter().map(|(_, b)| b).collect();
        assert_eq!(flags, vec![false, false, true]);
    }

    #[test]
    fn parse() {
        assert_eq!(parse_category(" 65 "), Some(65));
        assert_eq!(parse_category("x"), None);
    }
}
