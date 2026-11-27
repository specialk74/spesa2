//! User interface: rendered on the server, then hydrated in the browser.

use std::time::Duration;

use leptos::ev::{KeyboardEvent, SubmitEvent};
use leptos::prelude::*;
use leptos::task::spawn_local;
use leptos_meta::{MetaTags, Stylesheet, Title, provide_meta_context};
use leptos_router::components::{Route, Router, Routes};
use leptos_router::path;

use crate::icons::{IconClose, IconPlus};
use crate::api::{
    add_product, get_state, postpone_product, put_back, restore_product, set_category, take_product,
};
use crate::model::{
    Product, Removed, State, Suggested, category_color, category_label, describe_ago,
    describe_every, parse_category, suggest_categories, suggest_products, with_group_breaks,
};

const VERSION: &str = "4.0";

/// Server-Sent Events endpoint notifying database changes.
pub const EVENTS_URL: &str = "/api/events";
const MAX_SUGGESTIONS: usize = 15;

/// HTML document around the app.
pub fn shell(options: LeptosOptions) -> impl IntoView {
    view! {
        <!DOCTYPE html>
        <html lang="it">
            <head>
                <meta charset="utf-8"/>
                <meta name="viewport" content="width=device-width, initial-scale=1"/>
                <link
                    rel="stylesheet"
                    href="https://cdn.jsdelivr.net/npm/bootstrap@5.1.3/dist/css/bootstrap.min.css"
                    integrity="sha384-1BmE4kWBq78iYhFldvKuhfTAU6auU8tT94WrHftjDbrCEXSU1oBoqyl2QvZ6jIW3"
                    crossorigin="anonymous"
                />
                <AutoReload options=options.clone()/>
                <HydrationScripts options/>
                <MetaTags/>
            </head>
            <body>
                <App/>
            </body>
        </html>
    }
}

// ---------------------------------------------------------------------------
// Shared state

/// App-wide state, available to every component through the context.
#[derive(Clone, Copy)]
struct Ctx {
    /// Catalog + list, reloaded whenever `refresh` changes.
    state: Resource<Result<State, ServerFnError>>,
    refresh: RwSignal<u32>,
    /// Last error of a server call, shown under the navbar.
    error: RwSignal<Option<String>>,
}

impl Ctx {
    fn loaded(&self) -> State {
        self.state.get().and_then(Result::ok).unwrap_or_default()
    }

    /// Runs a server call, then reloads the state.
    fn mutate(self, call: impl Future<Output = Result<(), ServerFnError>> + 'static) {
        spawn_local(async move {
            self.error.set(call.await.err().map(|e| e.to_string()));
            self.refresh.update(|n| *n += 1);
        });
    }
}

fn ctx() -> Ctx {
    expect_context()
}

// ---------------------------------------------------------------------------
// App and pages

#[component]
pub fn App() -> impl IntoView {
    provide_meta_context();
    let refresh = RwSignal::new(0u32);
    let state = Resource::new(move || refresh.get(), |_| get_state());
    provide_context(Ctx { state, refresh, error: RwSignal::new(None) });
    // Effects run only in the browser: reload whenever the database changes.
    Effect::new(move |_| listen_db_changes(refresh));

    view! {
        <Stylesheet id="leptos" href="/pkg/spesa.css"/>
        <Title text="SpesaApp"/>
        <Router>
            <Routes fallback=|| view! { <p class="m-3">"Pagina non trovata"</p> }>
                <Route path=path!("/spesa") view=CatalogPage/>
                <Route path=path!("/spesa2") view=ListPage/>
            </Routes>
        </Router>
    }
}

/// Bumps `refresh` at every database change notified by the server.
///
/// The browser reconnects by itself if the connection drops (server restart,
/// phone waking up...): changes may have been missed meanwhile, so the data
/// is reloaded after every reconnection too.
#[cfg(feature = "hydrate")]
fn listen_db_changes(refresh: RwSignal<u32>) {
    use std::cell::Cell;
    use wasm_bindgen::JsCast;
    use wasm_bindgen::closure::Closure;

    let Ok(source) = web_sys::EventSource::new(EVENTS_URL) else {
        return;
    };
    let reload = move || refresh.update(|n| *n += 1);

    let on_change = Closure::<dyn Fn(web_sys::Event)>::new(move |_| reload());
    let _ = source.add_event_listener_with_callback("db", on_change.as_ref().unchecked_ref());

    let first_open = Cell::new(true);
    let on_open = Closure::<dyn Fn(web_sys::Event)>::new(move |_| {
        if !first_open.replace(false) {
            reload();
        }
    });
    source.set_onopen(Some(on_open.as_ref().unchecked_ref()));

    // The page lives as long as the app: keep the callbacks forever.
    on_change.forget();
    on_open.forget();
}

#[cfg(not(feature = "hydrate"))]
fn listen_db_changes(_refresh: RwSignal<u32>) {}

fn loading() -> impl IntoView {
    view! { <p class="m-3 text-muted">"Caricamento…"</p> }
}

/// `/spesa`: products not on the list (bought more than once), to put back on it.
#[component]
fn CatalogPage() -> impl IntoView {
    let ctx = ctx();
    let suggested = move || ctx.loaded().suggested;
    // the suggested products are shown only once, at the top
    let rows = move || {
        let state = ctx.loaded();
        let shown_above: Vec<i64> = state.suggested.iter().map(|s| s.product.id).collect();
        state
            .catalog
            .into_iter()
            .filter(|p| p.count > 1 && !shown_above.contains(&p.id))
            .collect::<Vec<_>>()
    };
    view! {
        <Header link="/spesa2"/>
        <NewProductForm/>
        <hr/>
        <Transition fallback=loading>
            {move || {
                (!suggested().is_empty())
                    .then(|| view! { <h2 class="section-title">"Da ricomprare"</h2> })
            }}
            <For each=suggested key=|s| s.clone() children=|s| view! { <SuggestedRow suggested=s/> }/>
            {move || {
                (!suggested().is_empty())
                    .then(|| view! { <h2 class="section-title">"Altri prodotti"</h2> })
            }}
            <For each=rows key=|p| p.clone() children=|p| view! { <CatalogRow product=p/> }/>
        </Transition>
    }
}

/// A product it is time to buy again: tapping the row puts it on the list,
/// the cross hides it until its next due time ("non ora").
#[component]
fn SuggestedRow(suggested: Suggested) -> impl IntoView {
    let ctx = ctx();
    let Suggested { product, every_days, days_ago } = suggested;
    let done = RwSignal::new(false);
    let (id, name, category) = (product.id, product.name.clone(), product.category);
    let add = move |ev: SubmitEvent| {
        ev.prevent_default();
        done.set(true);
        ctx.mutate(put_back(id, name.clone(), category));
    };
    let later = move |_| {
        done.set(true);
        ctx.mutate(postpone_product(id, every_days));
    };
    view! {
        <form class="input-group input-group-sm row-item suggested" class:d-none=done on:submit=add>
            <button class="form-control name row-btn" type="submit" title="Metti in lista">
                {product.name}
                <small class="freq">
                    {describe_every(every_days)} " · ultima volta " {describe_ago(days_ago)}
                </small>
            </button>
            <button
                class="btn btn-outline-secondary btn-icon btn-later"
                type="button"
                title="Non ora"
                on:click=later
            >
                <IconClose/>
            </button>
        </form>
    }
}

/// `/spesa2`: the shopping list, grouped and colored by category.
#[component]
fn ListPage() -> impl IntoView {
    let ctx = ctx();
    let rows = move || with_group_breaks(ctx.loaded().list);
    view! {
        <Header link="/spesa"/>
        <NewProductForm/>
        <hr/>
        <Transition fallback=loading>
            <For
                each=rows
                key=|row| row.clone()
                children=|(p, group_break)| view! { <ListRow product=p group_break/> }
            />
            <RemovedSection/>
        </Transition>
    }
}

// ---------------------------------------------------------------------------
// Components

/// Navbar (the title links to the other page) and error banner.
#[component]
fn Header(link: &'static str) -> impl IntoView {
    let ctx = ctx();
    let count = move || ctx.state.get().map(|s| s.map_or(0, |s| s.list.len()));
    let error = move || {
        ctx.error.get().or_else(|| {
            ctx.state.get().and_then(|s| s.err()).map(|e| e.to_string())
        })
    };
    view! {
        <nav class="navbar navbar-dark bg-dark">
            <div class="container-fluid">
                <a class="navbar-brand" href=link>
                    "SpesaApp - v " {VERSION} " - "
                    <Transition fallback=|| "…">{count}</Transition>
                    " elements"
                </a>
            </div>
        </nav>
        <Transition fallback=|| ()>
            {move || error().map(|e| view! {
                <div class="alert alert-danger m-1 py-1" on:click=move |_| ctx.error.set(None)>
                    {e}
                </div>
            })}
        </Transition>
    }
}

/// Adds a product (new or from the catalog) to the shopping list.
#[component]
fn NewProductForm() -> impl IntoView {
    let ctx = ctx();
    let name = RwSignal::new(String::new());
    let category = RwSignal::new(String::new());
    let invalid = RwSignal::new(false);

    let suggestions = Signal::derive(move || {
        let catalog = ctx.loaded().catalog;
        suggest_products(&catalog, &name.get(), MAX_SUGGESTIONS)
            .into_iter()
            .map(|p| Suggestion {
                label: p.name.clone(),
                hint: format!("{} · {}", p.category, category_label(p.category)),
                value: p.category.to_string(),
            })
            .collect()
    });
    let select = Callback::new(move |s: Suggestion| {
        name.set(s.label);
        category.set(s.value);
    });
    let submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        let Some(cat) = parse_category(&category.get_untracked()) else {
            invalid.set(true);
            return;
        };
        invalid.set(false);
        let product = name.get_untracked();
        if product.trim().is_empty() {
            return;
        }
        name.set(String::new());
        category.set(String::new());
        ctx.mutate(add_product(product, cat));
    };

    view! {
        <form class="input-group my-3 px-1" on:submit=submit>
            <button class="btn btn-warning btn-add" type="submit" title="Aggiungi">
                <IconPlus/>
            </button>
            <Autocomplete
                value=name
                suggestions
                on_select=select
                class="form-control"
                placeholder="Prodotto"
            />
            <CategoryInput value=category invalid placeholder="C: 0"/>
        </form>
    }
}

/// A catalog product: tapping the row (or Enter on the category) puts it back on the list.
#[component]
fn CatalogRow(product: Product) -> impl IntoView {
    let ctx = ctx();
    let id = product.id;
    let done = RwSignal::new(false);
    let name = product.name;
    let label = name.clone();
    let category = RwSignal::new(product.category.to_string());
    let invalid = RwSignal::new(false);

    let send = move || {
        let Some(cat) = parse_category(&category.get_untracked()) else {
            invalid.set(true);
            return;
        };
        invalid.set(false);
        done.set(true);
        ctx.mutate(put_back(id, name.clone(), cat));
    };
    let submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        send();
    };

    view! {
        <form class="input-group input-group-sm row-item" class:d-none=done on:submit=submit>
            <button class="form-control name row-btn" type="submit" title="Rimetti in lista">
                {label}
            </button>
            <span class="input-group-text tag">"C"</span>
            <CategoryInput value=category invalid/>
        </form>
    }
}

/// A product on the shopping list: tap the row when taken, Enter on the category saves it.
#[component]
fn ListRow(product: Product, group_break: bool) -> impl IntoView {
    let ctx = ctx();
    let id = product.id;
    let original = product.category;
    let done = RwSignal::new(false);
    let category = RwSignal::new(original.to_string());
    let invalid = RwSignal::new(false);

    let take = move |ev: SubmitEvent| {
        ev.prevent_default();
        done.set(true);
        ctx.mutate(take_product(id));
    };
    let save = Callback::new(move |()| match parse_category(&category.get_untracked()) {
        Some(cat) => {
            invalid.set(false);
            if cat != original {
                ctx.mutate(set_category(id, cat));
            }
        }
        None => invalid.set(true),
    });

    view! {
        {group_break.then(|| view! { <div class="group-gap"></div> })}
        <form class="input-group input-group-sm row-item" class:d-none=done on:submit=take>
            <button
                class="form-control fw-bold name row-btn"
                type="submit"
                title="Preso"
                style:color=category_color(original)
            >
                {product.name}
            </button>
            <CategoryInput value=category invalid on_commit=Some(save)/>
        </form>
    }
}

/// "Tolti di recente": products just taken off the list, to spot and undo mistakes.
#[component]
fn RemovedSection() -> impl IntoView {
    let ctx = ctx();
    let rows = move || ctx.loaded().removed;
    view! {
        {move || {
            (!rows().is_empty()).then(|| view! { <h2 class="section-title">"Tolti di recente"</h2> })
        }}
        <For each=rows key=|r| r.clone() children=|r| view! { <RemovedRow removed=r/> }/>
    }
}

/// A product recently taken: tapping the row puts it back on the list.
#[component]
fn RemovedRow(removed: Removed) -> impl IntoView {
    let ctx = ctx();
    let id = removed.product.id;
    let done = RwSignal::new(false);
    let submit = move |ev: SubmitEvent| {
        ev.prevent_default();
        done.set(true);
        ctx.mutate(restore_product(id));
    };
    view! {
        <form class="input-group input-group-sm row-item removed" class:d-none=done on:submit=submit>
            <button class="form-control name row-btn" type="submit" title="Rimetti in lista">
                {removed.product.name}
            </button>
            {(!removed.when.is_empty())
                .then(|| view! { <span class="input-group-text when">{removed.when}</span> })}
        </form>
    }
}

/// Category field with autocompletion of the reference categories.
#[component]
fn CategoryInput(
    value: RwSignal<String>,
    #[prop(into)] invalid: Signal<bool>,
    #[prop(default = None)] on_commit: Option<Callback<()>>,
    #[prop(into, optional)] placeholder: String,
) -> impl IntoView {
    let suggestions = Signal::derive(move || {
        suggest_categories(&value.get())
            .into_iter()
            .map(|(v, label)| Suggestion {
                label: label.to_string(),
                hint: v.to_string(),
                value: v.to_string(),
            })
            .collect()
    });
    let select = Callback::new(move |s: Suggestion| value.set(s.value));
    view! {
        <Autocomplete
            value
            suggestions
            on_select=select
            on_commit
            invalid
            class="form-control input-category"
            placeholder
            inputmode="numeric"
        />
    }
}

/// An entry of the autocompletion menu.
#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// Text shown in the menu.
    pub label: String,
    /// Secondary text shown in grey.
    pub hint: String,
    /// Payload passed to `on_select`.
    pub value: String,
}

/// Text input with a dropdown of suggestions (mouse/touch and keyboard).
///
/// Must be placed in a positioned container (e.g. a Bootstrap `.input-group`).
/// `on_commit`, if given, is called on Enter (and after choosing a suggestion)
/// instead of submitting the enclosing form.
#[component]
fn Autocomplete(
    value: RwSignal<String>,
    suggestions: Signal<Vec<Suggestion>>,
    on_select: Callback<Suggestion>,
    #[prop(default = None)] on_commit: Option<Callback<()>>,
    #[prop(into, default = Signal::stored(false))] invalid: Signal<bool>,
    #[prop(into)] class: String,
    #[prop(into, optional)] placeholder: String,
    #[prop(into, default = "text".to_string())] inputmode: String,
) -> impl IntoView {
    let open = RwSignal::new(false);
    let active = RwSignal::new(None::<usize>);

    let choose = move |s: Suggestion| {
        open.set(false);
        active.set(None);
        on_select.run(s);
        if let Some(commit) = on_commit {
            commit.run(());
        }
    };
    let step = move |delta: isize| {
        let n = suggestions.get_untracked().len() as isize;
        if n == 0 {
            return;
        }
        open.set(true);
        active.update(|a| {
            let current = a.map_or(if delta > 0 { -1 } else { n }, |i| i as isize);
            *a = Some((current + delta).rem_euclid(n) as usize);
        });
    };
    let keydown = move |ev: KeyboardEvent| match ev.key().as_str() {
        "ArrowDown" => {
            ev.prevent_default();
            step(1);
        }
        "ArrowUp" => {
            ev.prevent_default();
            step(-1);
        }
        "Escape" => open.set(false),
        "Enter" => {
            let highlighted = open
                .get_untracked()
                .then(|| active.get_untracked())
                .flatten()
                .and_then(|i| suggestions.get_untracked().get(i).cloned());
            if let Some(s) = highlighted {
                ev.prevent_default();
                choose(s);
            } else if let Some(commit) = on_commit {
                ev.prevent_default();
                open.set(false);
                commit.run(());
            } else {
                open.set(false);
            }
        }
        _ => {}
    };
    // Closing is delayed so that a tap on a suggestion is not lost on touch screens.
    let blur = move |_| {
        set_timeout(
            move || {
                open.try_set(false);
            },
            Duration::from_millis(200),
        )
    };

    let menu = move || {
        let items = suggestions.get();
        (open.get() && !items.is_empty()).then(|| {
            view! {
                <ul class="dropdown-menu show autocomplete-menu">
                    {items
                        .into_iter()
                        .enumerate()
                        .map(|(i, s)| {
                            let label = s.label.clone();
                            let hint = s.hint.clone();
                            view! {
                                <li>
                                    <button
                                        type="button"
                                        tabindex="-1"
                                        class="dropdown-item"
                                        class:active=move || active.get() == Some(i)
                                        on:mousedown=|ev| ev.prevent_default()
                                        on:click=move |_| choose(s.clone())
                                    >
                                        {label}
                                        <small class="hint">{hint}</small>
                                    </button>
                                </li>
                            }
                        })
                        .collect_view()}
                </ul>
            }
        })
    };

    view! {
        <input
            type="text"
            class=class
            class:is-invalid=invalid
            placeholder=placeholder
            inputmode=inputmode
            autocomplete="off"
            value=value.get_untracked()
            bind:value=value
            on:input=move |_| {
                open.set(true);
                active.set(None);
            }
            on:keydown=keydown
            on:blur=blur
        />
        {menu}
    }
}
