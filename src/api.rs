//! Server functions: called from the browser, executed on the server.

use leptos::prelude::*;

use crate::model::State;

/// Catalog and shopping list.
#[server]
pub async fn get_state() -> Result<State, ServerFnError> {
    crate::db::run(|conn| crate::db::load_state(conn)).await
}

/// Puts a product on the list by name (creating it if new).
#[server]
pub async fn add_product(name: String, category: i64) -> Result<(), ServerFnError> {
    crate::db::run(move |conn| crate::db::add_product(conn, &name, category)).await
}

/// Puts a catalog product back on the list; an empty name deletes it.
#[server]
pub async fn put_back(id: i64, name: String, category: i64) -> Result<(), ServerFnError> {
    crate::db::run(move |conn| crate::db::put_back(conn, id, &name, category)).await
}

/// Changes the category of a product.
#[server]
pub async fn set_category(id: i64, category: i64) -> Result<(), ServerFnError> {
    crate::db::run(move |conn| crate::db::set_category(conn, id, category)).await
}

/// Undoes "taken": puts the product back on the list.
#[server]
pub async fn restore_product(id: i64) -> Result<(), ServerFnError> {
    crate::db::run(move |conn| crate::db::restore(conn, id)).await
}

/// Marks a product as bought.
#[server]
pub async fn take_product(id: i64) -> Result<(), ServerFnError> {
    crate::db::run(move |conn| crate::db::take(conn, id)).await
}
