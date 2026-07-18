
use dioxus::prelude::*;

use crate::components::connection::use_connection_provider;
use crate::components::icons::Icon;
use crate::components::toast::Toaster;
use crate::views::{Groups, Hosts, Images, Tasks};

#[derive(Clone, Routable, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[rustfmt::skip]
pub enum Route {
    #[layout(Shell)]
        #[route("/")]
        Hosts {},
        #[route("/images")]
        Images {},
        #[route("/groups")]
        Groups {},
        #[route("/tasks")]
        Tasks {},
}

#[component]
pub fn App() -> Element {
    use_connection_provider();

    rsx! {
        document::Link { rel: "preconnect", href: "https://fonts.googleapis.com" }
        document::Link { rel: "preconnect", href: "https://fonts.gstatic.com" }
        document::Stylesheet { href: "https://fonts.googleapis.com/css2?family=Chakra+Petch:wght@500;600;700&family=IBM+Plex+Mono:wght@400;500&family=IBM+Plex+Sans:wght@400;500;600&display=swap" }
        document::Stylesheet { href: asset!("/assets/tailwind.css") }
        Router::<Route> {}
        Toaster {}
    }
}

#[component]
fn Shell() -> Element {
    rsx! {
        div { class: "min-h-screen flex",
            aside { class: "relative w-60 shrink-0 border-r border-line bg-ink-900/80 flex flex-col",
                div { class: "bg-grid absolute inset-0 pointer-events-none" }
                div { class: "relative z-10 flex h-full flex-col",
                    div { class: "border-b border-line px-5 py-5",
                        div { class: "font-display text-xl font-bold tracking-[0.15em] text-fog-100",
                            span { class: "text-amber-500", "im" }
                            "aged"
                        }
                        div { class: "mt-1 text-[10px] uppercase tracking-[0.3em] text-fog-600",
                            "netboot control"
                        }
                    }
                    nav { class: "flex flex-1 flex-col gap-1 px-3 py-4",
                        NavItem { to: Route::Hosts {}, icon: "host", label: "Hosts" }
                        NavItem { to: Route::Images {}, icon: "image", label: "Images" }
                        NavItem { to: Route::Groups {}, icon: "group", label: "Groups" }
                        NavItem { to: Route::Tasks {}, icon: "tasks", label: "Tasks" }
                    }
                    div { class: "border-t border-line px-5 py-4 font-mono text-[10px] text-fog-600",
                        "imaged · v0.1"
                    }
                }
            }
            main { class: "min-w-0 flex-1 overflow-y-auto",
                div { class: "mx-auto max-w-6xl px-8 py-8", Outlet::<Route> {} }
            }
        }
    }
}

#[component]
fn NavItem(to: Route, icon: &'static str, label: &'static str) -> Element {
    rsx! {
        Link {
            to,
            class: "group flex items-center gap-3 rounded-lg px-3 py-2 text-sm text-fog-400 transition-colors hover:bg-ink-800 hover:text-fog-100",
            active_class: "bg-ink-800 text-fog-100 glow-amber",
            Icon { name: icon, class: "w-4 h-4" }
            span { "{label}" }
        }
    }
}
