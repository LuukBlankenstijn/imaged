//! Unit tests for the pure, presentational components and formatting helpers,
//! rendered to HTML with `dioxus-ssr`.

use std::collections::{BTreeMap, HashSet};

use dioxus::prelude::*;

use crate::components::deploy_inputs::{
    deploy_request, erase_target, form_fields, initial_inputs, multicast_request,
    scripts_lack_root, submitted_inputs,
};
use crate::components::hooks::Polled;
use crate::components::icons::Icon;
use crate::components::partitions::{
    MatchBy, Resolution, Unfit, capture_sources, deploy_target, multicast_targets, resolve,
};
use crate::components::ui::{
    Button, EmptyState, ImageStatusBadge, StatusDot, TaskStateBadge, TaskTypeBadge,
};
use crate::format::{format_bytes, format_relative};
use crate::model::{
    DeployInputs, Host, HostPartition, Image, ImageKind, ImagePartition, ImageStatus, InputKind,
    InputSource, InputSources, Script, ScriptInput, TaskState, TaskType,
};
use crate::views::groups::{all_selected, toggle_visible};
use crate::views::scripts::InputDraft;

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
        (ImageStatus::Importing, "importing"),
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
        (ImageStatus::Importing, "text-run"),
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

#[test]
fn selecting_all_only_touches_the_hosts_the_search_leaves_visible() {
    let mut selected = HashSet::from([5]);
    let visible = [1, 2];

    toggle_visible(&mut selected, &visible);
    assert_eq!(selected, HashSet::from([1, 2, 5]));

    toggle_visible(&mut selected, &visible);
    assert_eq!(selected, HashSet::from([5]));
}

#[test]
fn the_select_all_control_flips_to_clear_only_when_every_visible_host_is_selected() {
    let all = HashSet::from([1, 2]);
    assert!(all_selected(&all, &[1, 2]));
    assert!(!all_selected(&HashSet::from([1]), &[1, 2]));
    assert!(!all_selected(&all, &[]));
}

#[test]
fn selecting_all_with_nothing_visible_changes_nothing() {
    let mut selected = HashSet::from([5]);
    toggle_visible(&mut selected, &[]);
    assert_eq!(selected, HashSet::from([5]));
}

fn input(name: &str, per_machine: bool, default_value: Option<&str>) -> ScriptInput {
    ScriptInput {
        name: name.to_string(),
        label: String::new(),
        description: None,
        kind: InputKind::Text,
        required: false,
        default_value: default_value.map(str::to_string),
        per_machine,
        choices: Vec::new(),
    }
}

fn script(id: i64, inputs: Vec<ScriptInput>) -> Script {
    Script {
        id,
        name: format!("script-{id}"),
        body: String::new(),
        updated_at: 0,
        inputs,
        root_partition: None,
    }
}

fn values(pairs: &[(&str, &str)]) -> InputSources {
    pairs
        .iter()
        .map(|(name, value)| (name.to_string(), InputSource::Value(value.to_string())))
        .collect()
}

#[test]
fn the_deploy_form_asks_for_an_input_shared_by_several_scripts_once_in_first_declaration_order() {
    let scripts = [
        script(
            1,
            vec![input("site", false, None), input("hostname", true, None)],
        ),
        script(
            2,
            vec![
                input("hostname", true, Some("pc")),
                input("role", false, None),
            ],
        ),
    ];

    let fields = form_fields(&scripts);

    let names: Vec<&str> = fields.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(names, ["site", "hostname", "role"]);
    assert_eq!(fields[1].default_value, None);
}

#[test]
fn the_deploy_form_prefills_defaults_shared_once_and_per_machine_for_every_host() {
    let fields = [
        input("site", false, Some("hq")),
        input("role", false, None),
        input("hostname", true, Some("pc")),
    ];

    let form = initial_inputs(&fields, &[4, 7]);

    assert_eq!(form.shared, values(&[("site", "hq")]));
    assert_eq!(
        form.per_machine,
        BTreeMap::from([
            (4, values(&[("hostname", "pc")])),
            (7, values(&[("hostname", "pc")]))
        ])
    );
}

#[test]
fn submitting_the_deploy_form_leaves_out_cleared_values_and_hosts_without_any() {
    let mut shared = values(&[("site", "hq"), ("role", "")]);
    shared.insert("key".into(), InputSource::Variable("HOST_KEY".into()));
    let form = DeployInputs {
        shared,
        per_machine: BTreeMap::from([
            (4, values(&[("hostname", "pc-4"), ("seat", "")])),
            (7, values(&[("hostname", "")])),
        ]),
    };

    let sent = submitted_inputs(&form);

    let mut expected = values(&[("site", "hq")]);
    expected.insert("key".into(), InputSource::Variable("HOST_KEY".into()));
    assert_eq!(sent.shared, expected);
    assert_eq!(
        sent.per_machine,
        BTreeMap::from([(4, values(&[("hostname", "pc-4")]))])
    );
}

#[test]
fn saving_an_unchanged_input_in_the_editor_keeps_its_choices_default_and_label_verbatim() {
    let input = ScriptInput {
        name: "edition".to_string(),
        label: " Edition ".to_string(),
        description: Some("Which edition to install".to_string()),
        kind: InputKind::Choice,
        required: true,
        default_value: Some(" pro".to_string()),
        per_machine: false,
        choices: vec![
            "home, office".to_string(),
            " pro".to_string(),
            "edu".to_string(),
        ],
    };

    let saved = ScriptInput::from(&InputDraft::from(&input));

    assert_eq!(saved, input);
}

#[test]
fn the_editor_takes_one_choice_per_line_and_skips_blank_lines() {
    let draft = InputDraft {
        name: " edition ".to_string(),
        label: "Edition".to_string(),
        description: String::new(),
        kind: InputKind::Choice,
        required: true,
        default_value: String::new(),
        per_machine: false,
        choices: "home, office\n\n pro\nedu\n".to_string(),
    };

    let saved = ScriptInput::from(&draft);

    assert_eq!(saved.name, "edition");
    assert_eq!(saved.default_value, None);
    assert_eq!(saved.choices, ["home, office", " pro", "edu"]);
}

#[test]
fn a_draft_switched_away_from_choice_drops_its_choices_and_an_empty_default() {
    let draft = InputDraft {
        name: "count".to_string(),
        label: String::new(),
        description: String::new(),
        kind: InputKind::Number,
        required: false,
        default_value: String::new(),
        per_machine: true,
        choices: "a\nb".to_string(),
    };

    let saved = ScriptInput::from(&draft);

    assert!(saved.choices.is_empty());
    assert_eq!(saved.default_value, None);
    assert!(saved.per_machine);
}

#[test]
fn a_failed_poll_keeps_the_last_loaded_value_and_reports_the_error() {
    let mut polled = Polled::default();
    polled.record(Ok::<_, String>(vec![1, 2]));

    polled.record(Err("server restarting".to_string()));

    assert_eq!(polled.value, Some(vec![1, 2]));
    assert_eq!(polled.error.as_deref(), Some("server restarting"));
}

#[test]
fn a_successful_poll_replaces_the_value_and_clears_the_error() {
    let mut polled = Polled::default();
    polled.record(Err::<Vec<i32>, _>("down".to_string()));
    assert_eq!(polled.value, None);

    polled.record(Ok::<_, String>(vec![3]));

    assert_eq!(polled.value, Some(vec![3]));
    assert_eq!(polled.error, None);
}

fn filled_form() -> DeployInputs {
    DeployInputs {
        shared: values(&[("site", "hq"), ("role", "")]),
        per_machine: BTreeMap::from([(4, values(&[("hostname", "pc-4")]))]),
    }
}

fn image(id: i64, kind: ImageKind, size_bytes: u64, fs_uuid: Option<&str>) -> Image {
    Image {
        id,
        name: format!("image-{id}"),
        captured_at: None,
        status: ImageStatus::Ready,
        error_message: None,
        partitions: vec![ImagePartition {
            id: 1,
            partition_number: 1,
            fstype: "ext4".to_string(),
            size_bytes,
            fs_uuid: fs_uuid.map(str::to_string),
            fs_label: None,
        }],
        root_partition: None,
        kind,
    }
}

fn partition(
    number: i64,
    partlabel: Option<&str>,
    fstype: Option<&str>,
    size_bytes: u64,
    uuid: Option<&str>,
) -> HostPartition {
    HostPartition {
        number,
        partuuid: format!("pu-{number}"),
        partlabel: partlabel.map(str::to_string),
        label: None,
        uuid: uuid.map(str::to_string),
        fstype: fstype.map(str::to_string),
        size_bytes,
    }
}

fn host_with(id: i64, partitions: Vec<HostPartition>) -> Host {
    Host {
        id,
        mac_address: format!("aa:bb:cc:dd:ee:{id:02x}"),
        name: format!("lab-{id}"),
        disk_size_bytes: Some(1 << 40),
        disk_error: None,
        ip: None,
        partitions,
    }
}

#[test]
fn a_deploy_cannot_be_submitted_before_an_image_is_chosen_and_its_inputs_are_loaded() {
    let form = filled_form();
    let disk = image(9, ImageKind::Disk, 1, None);

    assert_eq!(deploy_request(4, None, None, Some(&form)), None);
    assert_eq!(deploy_request(4, Some(&disk), None, None), None);

    let request = deploy_request(4, Some(&disk), Some("pu-2"), Some(&form)).unwrap();
    assert_eq!((request.id, request.image_id), (4, 9));
    assert_eq!(request.inputs, submitted_inputs(&form));
    assert_eq!(request.partition, None);
}

#[test]
fn a_partition_image_deploy_needs_a_target_partition_and_sends_it() {
    let form = filled_form();
    let single = image(9, ImageKind::Partition, 1, None);

    assert_eq!(deploy_request(4, Some(&single), None, Some(&form)), None);

    let request = deploy_request(4, Some(&single), Some("pu-2"), Some(&form)).unwrap();
    assert_eq!(request.partition.as_deref(), Some("pu-2"));
}

#[test]
fn a_multicast_targets_every_member_and_needs_members_an_image_and_loaded_inputs() {
    let form = filled_form();
    let disk = image(9, ImageKind::Disk, 1, None);

    assert_eq!(multicast_request(&[], Some(&disk), None, Some(&form)), None);
    assert_eq!(multicast_request(&[4, 7], None, None, Some(&form)), None);
    assert_eq!(multicast_request(&[4, 7], Some(&disk), None, None), None);

    let request = multicast_request(
        &[4, 7],
        Some(&disk),
        Some(BTreeMap::from([(4, "pu-1".to_string())])),
        Some(&form),
    )
    .unwrap();
    assert_eq!(request.host_ids, [4, 7]);
    assert_eq!(request.image_id, 9);
    assert_eq!(request.inputs.shared, values(&[("site", "hq")]));
    assert!(request.partitions.is_empty());
}

#[test]
fn a_partition_multicast_is_blocked_until_every_host_resolves_and_sends_the_map() {
    let form = filled_form();
    let single = image(9, ImageKind::Partition, 100, None);
    let hosts = [
        host_with(4, vec![partition(2, Some("root"), Some("ext4"), 100, None)]),
        host_with(
            7,
            vec![
                partition(1, Some("esp"), Some("vfat"), 500, None),
                partition(3, Some("root"), Some("ext4"), 200, None),
            ],
        ),
    ];

    let by_label = multicast_targets(&single, &hosts, MatchBy::Label, " root ");
    assert_eq!(
        by_label,
        Some(BTreeMap::from([
            (4, "pu-2".to_string()),
            (7, "pu-3".to_string())
        ]))
    );
    assert_eq!(
        multicast_targets(&single, &hosts, MatchBy::Number, "2"),
        None
    );
    assert_eq!(multicast_targets(&single, &hosts, MatchBy::Label, ""), None);
    assert_eq!(
        multicast_request(&[4, 7], Some(&single), None, Some(&form)),
        None
    );
    let request = multicast_request(&[4, 7], Some(&single), by_label.clone(), Some(&form)).unwrap();
    assert_eq!(Some(request.partitions), by_label);
}

#[test]
fn a_matched_partition_resolves_only_when_it_is_unique_large_enough_and_holds_no_twin() {
    let single = image(9, ImageKind::Partition, 100, Some("ABCD-1234"));
    let host = host_with(
        4,
        vec![
            partition(1, Some("data"), Some("ext4"), 50, None),
            partition(2, Some("root"), Some("ext4"), 100, Some("abcd-1234")),
            partition(3, Some("root"), Some("ext4"), 300, None),
            partition(4, Some("spare"), None, 300, None),
        ],
    );

    assert_eq!(
        resolve(&single, &host, MatchBy::Number, "2"),
        Resolution::Target(host.partitions[1].clone())
    );
    assert_eq!(
        resolve(&single, &host, MatchBy::Number, "1"),
        Resolution::Unfit(host.partitions[0].clone(), Unfit::TooSmall { needed: 100 })
    );
    assert_eq!(
        resolve(&single, &host, MatchBy::Label, "spare"),
        Resolution::Unfit(
            host.partitions[3].clone(),
            Unfit::HoldsFilesystem {
                twin: "#2 (root)".to_string()
            }
        )
    );
    assert_eq!(
        resolve(&single, &host, MatchBy::Label, "root"),
        Resolution::Ambiguous(2)
    );
    assert_eq!(
        resolve(&single, &host, MatchBy::Label, "ROOT"),
        Resolution::NoMatch
    );
    assert_eq!(
        resolve(&single, &host, MatchBy::Number, "x"),
        Resolution::NoMatch
    );
    assert_eq!(
        deploy_target(&single, &host, "pu-2"),
        Some(host.partitions[1].clone())
    );
    assert_eq!(deploy_target(&single, &host, "pu-1"), None);
    assert_eq!(deploy_target(&single, &host, "pu-4"), None);
}

#[test]
fn a_partition_beside_one_holding_the_images_filesystem_label_is_unfit() {
    let mut single = image(9, ImageKind::Partition, 100, None);
    single.partitions[0].fs_label = Some("root fs".to_string());
    let labelled = |number, label: &str| {
        let mut partition = partition(number, Some("root"), Some("ext4"), 100, None);
        partition.label = Some(label.to_string());
        partition
    };
    let twin = labelled(2, "root fs");
    let other_case = labelled(5, "ROOT FS");
    let target = partition(3, Some("spare"), None, 300, None);
    let host = host_with(4, vec![twin.clone(), target.clone()]);

    let unfit = Unfit::HoldsLabel {
        twin: "#2 (root)".to_string(),
        label: "root fs".to_string(),
    };
    assert_eq!(
        resolve(&single, &host, MatchBy::Label, "spare"),
        Resolution::Unfit(target.clone(), unfit.clone())
    );
    assert_eq!(
        unfit.reason(),
        "#2 (root) already holds a filesystem labelled `root fs`"
    );
    assert_eq!(deploy_target(&single, &host, "pu-2"), Some(twin));

    let host = host_with(4, vec![other_case, target.clone()]);
    assert_eq!(deploy_target(&single, &host, "pu-3"), Some(target));
}

#[test]
fn only_ext_vfat_and_btrfs_partitions_can_be_captured() {
    let host = host_with(
        4,
        vec![
            partition(1, None, Some("vfat"), 1, None),
            partition(2, None, Some("swap"), 1, None),
            partition(3, None, Some("ext4"), 1, None),
            partition(4, None, Some("ntfs"), 1, None),
            partition(5, None, None, 1, None),
            partition(6, None, Some("btrfs"), 1, None),
        ],
    );

    let numbers: Vec<i64> = capture_sources(&host).iter().map(|p| p.number).collect();

    assert_eq!(numbers, [1, 3, 6]);
}

#[test]
fn scripts_resolve_their_root_from_their_own_pick_or_the_image_root() {
    let mut disk = image(9, ImageKind::Disk, 1, None);
    let mut rooted = script(1, Vec::new());
    rooted.root_partition = Some(1);
    let unrooted = script(2, Vec::new());

    assert!(!scripts_lack_root(&disk, &[]));
    assert!(!scripts_lack_root(&disk, std::slice::from_ref(&rooted)));
    assert!(scripts_lack_root(
        &disk,
        &[rooted.clone(), unrooted.clone()]
    ));

    disk.root_partition = Some(1);
    assert!(!scripts_lack_root(&disk, &[rooted, unrooted]));
}

#[test]
fn the_erase_warning_names_each_target_with_its_mac_and_disk() {
    let mut host = Host {
        id: 4,
        mac_address: "aa:bb:cc:dd:ee:04".to_string(),
        name: "lab-4".to_string(),
        disk_size_bytes: Some(1024u64.pow(3)),
        disk_error: None,
        ip: None,
        partitions: Vec::new(),
    };
    assert_eq!(erase_target(&host), "lab-4 · aa:bb:cc:dd:ee:04 · 1.00 GiB");

    host.disk_size_bytes = None;
    host.disk_error = Some("multiple disks found".to_string());
    assert_eq!(
        erase_target(&host),
        "lab-4 · aa:bb:cc:dd:ee:04 · no usable disk"
    );
}
