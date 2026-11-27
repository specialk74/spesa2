//! Icons drawn as inline SVG: no image files to ship, sharp at any resolution.
//! They use `currentColor`, so their color comes from the CSS of the button.

use leptos::prelude::*;

/// 24×24 stroked icon made of the given SVG path.
#[component]
fn Icon(path: &'static str) -> impl IntoView {
    view! {
        <svg
            class="icon"
            viewBox="0 0 24 24"
            fill="none"
            stroke="currentColor"
            stroke-width="3"
            stroke-linecap="round"
            stroke-linejoin="round"
            aria-hidden="true"
        >
            <path d=path/>
        </svg>
    }
}

/// Cross: dismiss.
#[component]
pub fn IconClose() -> impl IntoView {
    view! { <Icon path="M6.5 6.5l11 11M17.5 6.5l-11 11"/> }
}

/// Plus: add a product.
#[component]
pub fn IconPlus() -> impl IntoView {
    view! { <Icon path="M12 4.5v15M4.5 12h15"/> }
}
