use dioxus::prelude::*;

#[component]
pub fn Icon(name: &'static str, #[props(default)] class: String) -> Element {
    let class = if class.is_empty() {
        "w-4 h-4".to_string()
    } else {
        class
    };
    let body = match name {
        "host" => rsx! {
            rect { x: "3", y: "4", width: "18", height: "6", rx: "1.5" }
            rect { x: "3", y: "14", width: "18", height: "6", rx: "1.5" }
            line { x1: "7", y1: "7", x2: "7.01", y2: "7" }
            line { x1: "7", y1: "17", x2: "7.01", y2: "17" }
        },
        "image" => rsx! {
            circle { cx: "12", cy: "12", r: "8.5" }
            circle { cx: "12", cy: "12", r: "2.5" }
        },
        "group" => rsx! {
            polygon { points: "12,3 21,7.5 12,12 3,7.5" }
            polyline { points: "3,12 12,16.5 21,12" }
            polyline { points: "3,16.5 12,21 21,16.5" }
        },
        "tasks" => rsx! {
            line { x1: "9", y1: "6", x2: "20", y2: "6" }
            line { x1: "9", y1: "12", x2: "20", y2: "12" }
            line { x1: "9", y1: "18", x2: "20", y2: "18" }
            polyline { points: "3,6 4,7 6,5" }
            polyline { points: "3,12 4,13 6,11" }
            polyline { points: "3,18 4,19 6,17" }
        },
        "deploy" => rsx! {
            path { d: "M12 3v12" }
            polyline { points: "7,10 12,15 17,10" }
            path { d: "M4 20h16" }
        },
        "capture" => rsx! {
            path { d: "M12 21V9" }
            polyline { points: "7,14 12,9 17,14" }
            path { d: "M4 4h16" }
        },
        "multicast" => rsx! {
            circle { cx: "12", cy: "12", r: "2" }
            path { d: "M8.5 8.5a5 5 0 0 0 0 7" }
            path { d: "M15.5 8.5a5 5 0 0 1 0 7" }
            path { d: "M5.5 5.5a9 9 0 0 0 0 13" }
            path { d: "M18.5 5.5a9 9 0 0 1 0 13" }
        },
        "reboot" => rsx! {
            path { d: "M3 12a9 9 0 1 0 3-6.7" }
            polyline { points: "3,4 3,9 8,9" }
        },
        "wake" => rsx! {
            path { d: "M12 3v9" }
            path { d: "M6.6 6.6a8 8 0 1 0 10.8 0" }
        },
        "trash" => rsx! {
            polyline { points: "4,7 20,7" }
            path { d: "M9 7V4h6v3" }
            path { d: "M6 7l1 13h10l1-13" }
        },
        "edit" => rsx! {
            path { d: "M4 20h4l10-10-4-4L4 16v4z" }
            line { x1: "13.5", y1: "6.5", x2: "17.5", y2: "10.5" }
        },
        "kebab" => rsx! {
            circle { cx: "12", cy: "5", r: "1.4" }
            circle { cx: "12", cy: "12", r: "1.4" }
            circle { cx: "12", cy: "19", r: "1.4" }
        },
        "close" => rsx! {
            line { x1: "6", y1: "6", x2: "18", y2: "18" }
            line { x1: "18", y1: "6", x2: "6", y2: "18" }
        },
        "check" => rsx! {
            polyline { points: "4,12 10,18 20,6" }
        },
        "alert" => rsx! {
            path { d: "M12 3l9 16H3z" }
            line { x1: "12", y1: "9", x2: "12", y2: "14" }
            line { x1: "12", y1: "17", x2: "12.01", y2: "17" }
        },
        "search" => rsx! {
            circle { cx: "11", cy: "11", r: "7" }
            line { x1: "16.5", y1: "16.5", x2: "21", y2: "21" }
        },
        "plus" => rsx! {
            line { x1: "12", y1: "5", x2: "12", y2: "19" }
            line { x1: "5", y1: "12", x2: "19", y2: "12" }
        },
        "chevron" => rsx! {
            polyline { points: "6,9 12,15 18,9" }
        },
        _ => rsx! {
            circle { cx: "12", cy: "12", r: "9" }
        },
    };
    rsx! {
        svg {
            class: "{class}",
            "viewBox": "0 0 24 24",
            fill: "none",
            stroke: "currentColor",
            "stroke-width": "1.8",
            "stroke-linecap": "round",
            "stroke-linejoin": "round",
            {body}
        }
    }
}
