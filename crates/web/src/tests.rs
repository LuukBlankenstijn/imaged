//! Unit tests for the pure, presentational components and formatting helpers,
//! rendered to HTML with `dioxus-ssr`.

use dioxus::prelude::*;

use crate::components::icons::Icon;
use crate::components::ui::{
    Button, EmptyState, ImageStatusBadge, StatusDot, TaskStateBadge, TaskTypeBadge,
};
use crate::format::{format_bytes, format_relative};
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
// Badges: every enum variant renders its own label
// ---------------------------------------------------------------------------

#[test]
fn image_status_badge_labels_every_status() {
    for (status, label) in [
        (ImageStatus::Ready, "ready"),
        (ImageStatus::Capturing, "capturing"),
        (ImageStatus::Faulted, "faulted"),
        (ImageStatus::Empty, "empty"),
    ] {
        let h = render(rsx! { ImageStatusBadge { status } });
        assert!(h.contains(label), "{status:?} rendered {h}");
    }
}

#[test]
fn task_state_badge_labels_every_state() {
    for (state, label) in [
        (TaskState::Done, "done"),
        (TaskState::Running, "running"),
        (TaskState::Pending, "pending"),
        (TaskState::Failed, "failed"),
        (TaskState::Partial, "partial"),
        (TaskState::Cancelled, "cancelled"),
    ] {
        let h = render(rsx! { TaskStateBadge { state } });
        assert!(h.contains(label), "{state:?} rendered {h}");
    }
}

#[test]
fn task_type_badge_labels_every_type() {
    for (task_type, label) in [
        (TaskType::Capture, "capture"),
        (TaskType::Deploy, "deploy"),
        (TaskType::Multicast, "multicast"),
        (TaskType::Reboot, "reboot"),
    ] {
        let h = render(rsx! { TaskTypeBadge { r#type: task_type } });
        assert!(h.contains(label), "{task_type:?} rendered {h}");
        assert!(h.contains("<svg"), "{task_type:?} rendered no icon");
    }
}

#[test]
fn status_dot_labels_both_connection_states() {
    let connected = render(rsx! { StatusDot { connected: true } });
    assert!(connected.contains("connected"), "{connected}");

    let disconnected = render(rsx! { StatusDot { connected: false } });
    assert!(disconnected.contains("disconnected"), "{disconnected}");
}

// ---------------------------------------------------------------------------
// Button / Icon behaviour
// ---------------------------------------------------------------------------

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
fn icon_unknown_name_falls_back_to_circle() {
    let h = render(rsx! { Icon { name: "definitely-not-an-icon" } });
    assert!(h.contains("<svg"), "{h}");
    assert!(h.contains("<circle"), "{h}");
}

// ---------------------------------------------------------------------------
// format_relative
// ---------------------------------------------------------------------------

#[test]
fn format_relative_returns_empty_without_a_browser_clock() {
    for millis in [0i64, 1_000, -1_000, 1_700_000_000_000, i64::MAX, i64::MIN] {
        assert_eq!(format_relative(millis), "");
    }
}

// ---------------------------------------------------------------------------
// Badge tone mapping and empty-state branch
// ---------------------------------------------------------------------------

#[test]
fn image_status_badge_maps_every_status_to_its_tone_class() {
    for (status, tone_class) in [
        (ImageStatus::Ready, "text-ok"),
        (ImageStatus::Capturing, "text-warn"),
        (ImageStatus::Faulted, "text-bad"),
        (ImageStatus::Empty, "text-idle"),
    ] {
        let h = render(rsx! { ImageStatusBadge { status } });
        assert!(h.contains(tone_class), "{status:?} rendered {h}");
    }
}

#[test]
fn task_state_badge_maps_every_state_to_its_tone_class() {
    for (state, tone_class) in [
        (TaskState::Done, "text-ok"),
        (TaskState::Running, "text-run"),
        (TaskState::Pending, "text-warn"),
        (TaskState::Failed, "text-bad"),
        (TaskState::Partial, "text-warn"),
        (TaskState::Cancelled, "text-idle"),
    ] {
        let h = render(rsx! { TaskStateBadge { state } });
        assert!(h.contains(tone_class), "{state:?} rendered {h}");
    }
}

#[test]
fn empty_state_renders_the_hint_paragraph_only_when_a_hint_is_present() {
    let with_hint = render(rsx! { EmptyState { title: "No images", hint: "Capture one first." } });
    assert!(with_hint.contains("Capture one first."), "{with_hint}");
    assert!(with_hint.contains("<p"), "{with_hint}");

    let without_hint = render(rsx! { EmptyState { title: "No images" } });
    assert!(without_hint.contains("No images"), "{without_hint}");
    assert!(!without_hint.contains("<p"), "{without_hint}");
}
