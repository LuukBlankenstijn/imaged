//! Unit tests for the pure, presentational components and formatting helpers.
//!
//! Components are rendered to HTML server-side via `dioxus-ssr` (native) and we
//! assert on the real labels / CSS classes / SVG shapes they emit. Only
//! hook-free, context-free components are covered here.

use dioxus::prelude::*;

use crate::components::icons::Icon;
use crate::components::ui::{
    Badge, Button, ButtonVariant, Card, EmptyState, ImageStatusBadge, PageHeader, StatusDot,
    TaskStateBadge, TaskTypeBadge, Tone,
};
use crate::format::format_bytes;
use crate::model::{ImageStatus, TaskState, TaskType};

fn render(el: Element) -> String {
    dioxus_ssr::render_element(el)
}

/// Render a zero-arg component inside a real `VirtualDom` so its `rsx!` (and any
/// prop defaults that need the Dioxus runtime, e.g. `EventHandler`) is built
/// within a runtime scope.
fn render_scoped(app: fn() -> Element) -> String {
    let mut dom = VirtualDom::new(app);
    dom.rebuild_in_place();
    dioxus_ssr::render(&dom)
}

// ---------------------------------------------------------------------------
// format_bytes
// ---------------------------------------------------------------------------

#[test]
fn format_bytes_zero() {
    assert_eq!(format_bytes(0), "0 B");
}

#[test]
fn format_bytes_raw_bytes_have_no_decimals() {
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(1023), "1023 B");
}

#[test]
fn format_bytes_kib_boundary() {
    assert_eq!(format_bytes(1024), "1.00 KiB");
}

#[test]
fn format_bytes_mib_rounds_to_two_decimals() {
    assert_eq!(format_bytes(1024 * 1024), "1.00 MiB");
    assert_eq!(format_bytes(1024 * 1024 * 3 / 2), "1.50 MiB");
}

#[test]
fn format_bytes_gib_and_tib() {
    assert_eq!(format_bytes(1024u64.pow(3)), "1.00 GiB");
    assert_eq!(format_bytes(1024u64.pow(4)), "1.00 TiB");
}

// ---------------------------------------------------------------------------
// ImageStatusBadge: status -> (label, tone class)
// ---------------------------------------------------------------------------

#[test]
fn image_status_badge_ready() {
    let h = render(rsx! { ImageStatusBadge { status: ImageStatus::Ready } });
    assert!(h.contains("ready"), "{h}");
    assert!(h.contains("text-ok"), "{h}");
}

#[test]
fn image_status_badge_capturing() {
    let h = render(rsx! { ImageStatusBadge { status: ImageStatus::Capturing } });
    assert!(h.contains("capturing"), "{h}");
    assert!(h.contains("text-warn"), "{h}");
}

#[test]
fn image_status_badge_faulted() {
    let h = render(rsx! { ImageStatusBadge { status: ImageStatus::Faulted } });
    assert!(h.contains("faulted"), "{h}");
    assert!(h.contains("text-bad"), "{h}");
}

#[test]
fn image_status_badge_empty() {
    let h = render(rsx! { ImageStatusBadge { status: ImageStatus::Empty } });
    assert!(h.contains("empty"), "{h}");
    assert!(h.contains("text-idle"), "{h}");
}

// ---------------------------------------------------------------------------
// TaskStateBadge: every state renders its label
// ---------------------------------------------------------------------------

#[test]
fn task_state_badge_done() {
    let h = render(rsx! { TaskStateBadge { state: TaskState::Done } });
    assert!(h.contains("done"), "{h}");
    assert!(h.contains("text-ok"), "{h}");
}

#[test]
fn task_state_badge_running() {
    let h = render(rsx! { TaskStateBadge { state: TaskState::Running } });
    assert!(h.contains("running"), "{h}");
    assert!(h.contains("text-run"), "{h}");
}

#[test]
fn task_state_badge_pending() {
    let h = render(rsx! { TaskStateBadge { state: TaskState::Pending } });
    assert!(h.contains("pending"), "{h}");
    assert!(h.contains("text-warn"), "{h}");
}

#[test]
fn task_state_badge_failed() {
    let h = render(rsx! { TaskStateBadge { state: TaskState::Failed } });
    assert!(h.contains("failed"), "{h}");
    assert!(h.contains("text-bad"), "{h}");
}

#[test]
fn task_state_badge_partial() {
    let h = render(rsx! { TaskStateBadge { state: TaskState::Partial } });
    assert!(h.contains("partial"), "{h}");
    assert!(h.contains("text-warn"), "{h}");
}

#[test]
fn task_state_badge_cancelled() {
    let h = render(rsx! { TaskStateBadge { state: TaskState::Cancelled } });
    assert!(h.contains("cancelled"), "{h}");
    assert!(h.contains("text-idle"), "{h}");
}

// ---------------------------------------------------------------------------
// TaskTypeBadge: neutral tone + icon + label per type
// ---------------------------------------------------------------------------

#[test]
fn task_type_badge_capture() {
    let h = render(rsx! { TaskTypeBadge { r#type: TaskType::Capture } });
    assert!(h.contains("capture"), "{h}");
    assert!(h.contains("<svg"), "{h}");
    assert!(h.contains("text-fog-400"), "{h}");
}

#[test]
fn task_type_badge_deploy() {
    let h = render(rsx! { TaskTypeBadge { r#type: TaskType::Deploy } });
    assert!(h.contains("deploy"), "{h}");
    assert!(h.contains("<svg"), "{h}");
}

#[test]
fn task_type_badge_multicast() {
    let h = render(rsx! { TaskTypeBadge { r#type: TaskType::Multicast } });
    assert!(h.contains("multicast"), "{h}");
}

#[test]
fn task_type_badge_reboot() {
    let h = render(rsx! { TaskTypeBadge { r#type: TaskType::Reboot } });
    assert!(h.contains("reboot"), "{h}");
}

// ---------------------------------------------------------------------------
// StatusDot: connected vs disconnected produce distinct output
// ---------------------------------------------------------------------------

#[test]
fn status_dot_connected() {
    let h = render(rsx! { StatusDot { connected: true } });
    assert!(h.contains("bg-ok"), "{h}");
    assert!(h.contains("pulse-live"), "{h}");
    assert!(h.contains("connected"), "{h}");
}

#[test]
fn status_dot_disconnected() {
    let h = render(rsx! { StatusDot { connected: false } });
    assert!(h.contains("bg-idle/50"), "{h}");
    assert!(h.contains("disconnected"), "{h}");
    assert!(!h.contains("pulse-live"), "{h}");
}

// ---------------------------------------------------------------------------
// Badge / Button / Card / EmptyState / PageHeader
// ---------------------------------------------------------------------------

#[test]
fn badge_renders_children_and_tone() {
    let h = render(rsx! { Badge { tone: Tone::Ok, "LIVE" } });
    assert!(h.contains("LIVE"), "{h}");
    assert!(h.contains("text-ok"), "{h}");
}

#[test]
fn button_default_variant_is_primary_with_children() {
    fn app() -> Element {
        rsx! { Button { "Go" } }
    }
    let h = render_scoped(app);
    assert!(h.contains("<button"), "{h}");
    assert!(h.contains("bg-amber-500"), "{h}");
    assert!(h.contains("Go"), "{h}");
}

#[test]
fn button_variant_classes_differ() {
    fn ghost_app() -> Element {
        rsx! { Button { variant: ButtonVariant::Ghost, "x" } }
    }
    fn danger_app() -> Element {
        rsx! { Button { variant: ButtonVariant::Danger, "x" } }
    }
    let ghost = render_scoped(ghost_app);
    assert!(ghost.contains("border-ink-600"), "{ghost}");
    let danger = render_scoped(danger_app);
    assert!(danger.contains("text-bad"), "{danger}");
}

#[test]
fn button_disabled_attribute_reflects_prop() {
    fn on_app() -> Element {
        rsx! { Button { disabled: true, "x" } }
    }
    fn off_app() -> Element {
        rsx! { Button { disabled: false, "x" } }
    }
    let on = render_scoped(on_app);
    assert!(on.contains("disabled=true"), "{on}");
    let off = render_scoped(off_app);
    assert!(!off.contains("disabled=true"), "{off}");
}

#[test]
fn card_renders_children() {
    let h = render(rsx! { Card { "inside" } });
    assert!(h.contains("inside"), "{h}");
}

#[test]
fn empty_state_shows_title_and_hint() {
    let h = render(rsx! { EmptyState { title: "No hosts", hint: "connect one" } });
    assert!(h.contains("No hosts"), "{h}");
    assert!(h.contains("connect one"), "{h}");
}

#[test]
fn empty_state_without_hint_shows_title() {
    let h = render(rsx! { EmptyState { title: "Nothing here" } });
    assert!(h.contains("Nothing here"), "{h}");
}

#[test]
fn page_header_shows_title_and_subtitle() {
    let h = render(rsx! { PageHeader { title: "Hosts", subtitle: "all machines" } });
    assert!(h.contains("<h1"), "{h}");
    assert!(h.contains("Hosts"), "{h}");
    assert!(h.contains("all machines"), "{h}");
}

// ---------------------------------------------------------------------------
// Icon: known name -> specific shape, unknown -> fallback circle
// ---------------------------------------------------------------------------

#[test]
fn icon_known_name_renders_its_shape() {
    let h = render(rsx! { Icon { name: "check" } });
    assert!(h.contains("<svg"), "{h}");
    assert!(h.contains(r#"points="4,12 10,18 20,6""#), "{h}");
}

#[test]
fn icon_unknown_name_falls_back_to_circle() {
    let h = render(rsx! { Icon { name: "definitely-not-an-icon" } });
    assert!(h.contains("<svg"), "{h}");
    assert!(h.contains("<circle"), "{h}");
    assert!(h.contains(r#"r="9""#), "{h}");
}
