use imaged_core::domain::{
    host::Host as DHost,
    image::Image as DImage,
    script::{NewScript as DNewScript, Script as DScript},
    task::{ScriptRun as DScriptRun, Task as DTask, TaskHost as DTaskHost, TaskType as DType},
};
use imaged_core::multicast::MulticastProgress as DProgress;
use imaged_model::script::ScriptInput as DScriptInput;

use crate::model;

impl From<DHost> for model::Host {
    fn from(h: DHost) -> Self {
        Self {
            id: h.id,
            mac_address: h.mac_address,
            name: h.name,
            disk_size_bytes: h.disk_size,
            ip: h.ip,
        }
    }
}

impl From<DProgress> for model::MulticastProgress {
    fn from(p: DProgress) -> Self {
        Self {
            task_id: p.task_id,
            phase: p.phase,
            fraction: p.fraction,
            bytes_per_second: p.bytes_per_second,
            step: p.step,
            steps: p.steps,
            eta_seconds: p.eta.map(|eta| eta.as_secs()),
            hosts: p.hosts,
        }
    }
}

impl From<DImage> for model::Image {
    fn from(i: DImage) -> Self {
        Self {
            id: i.id,
            name: i.name,
            captured_at: i.captured_at.map(|t| t.timestamp_millis()),
            status: i.status,
            error_message: i.error,
            partitions: i.partitions,
            root_partition: i.root_partition,
        }
    }
}

impl From<DType> for model::TaskType {
    fn from(t: DType) -> Self {
        match t {
            DType::Capture => Self::Capture,
            DType::Deploy => Self::Deploy,
            DType::Multicast => Self::Multicast,
            DType::Reboot => Self::Reboot,
        }
    }
}

impl From<DTaskHost> for model::TaskHost {
    fn from(h: DTaskHost) -> Self {
        Self {
            host_id: h.host_id,
            state: h.state,
            error: h.error,
            started_at: h.started_at.map(|t| t.timestamp_millis()),
            finished_at: h.finished_at.map(|t| t.timestamp_millis()),
        }
    }
}

impl From<DTask> for model::Task {
    fn from(t: DTask) -> Self {
        let state = t.aggregate_state();
        Self {
            id: t.id,
            r#type: t.task_type.into(),
            state,
            hosts: t.hosts.into_iter().map(Into::into).collect(),
            image_id: t.image_id,
            created_at: t.created_at.timestamp_millis(),
            image_name: t.image_name,
            image_deleted: t.image_deleted,
        }
    }
}

impl From<DScriptInput> for model::ScriptInput {
    fn from(i: DScriptInput) -> Self {
        Self {
            name: i.name,
            label: i.label,
            kind: i.kind,
            required: i.required,
            default_value: i.default_value,
            per_machine: i.per_machine,
            choices: i.choices,
        }
    }
}

impl From<model::ScriptInput> for DScriptInput {
    fn from(i: model::ScriptInput) -> Self {
        Self {
            name: i.name,
            label: i.label,
            kind: i.kind,
            required: i.required,
            default_value: i.default_value,
            per_machine: i.per_machine,
            choices: i.choices,
        }
    }
}

impl From<DScript> for model::Script {
    fn from(s: DScript) -> Self {
        Self {
            id: s.id,
            name: s.name,
            body: s.body,
            updated_at: s.updated_at.timestamp_millis(),
            inputs: s.inputs.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<model::ScriptRequest> for DNewScript {
    fn from(s: model::ScriptRequest) -> Self {
        Self {
            name: s.name,
            body: s.body,
            inputs: s.inputs.into_iter().map(Into::into).collect(),
        }
    }
}

impl From<DScriptRun> for model::ScriptRun {
    fn from(r: DScriptRun) -> Self {
        Self {
            position: r.position,
            script_name: r.script_name,
            output: r.output,
            exit_code: r.exit_code,
            started_at: r.started_at.timestamp_millis(),
            finished_at: r.finished_at.map(|t| t.timestamp_millis()),
        }
    }
}
