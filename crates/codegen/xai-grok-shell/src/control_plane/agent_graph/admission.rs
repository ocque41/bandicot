use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum AdmissionClass {
    Compensation,
    Interactive,
    Ultra,
    Graph,
    Swarm,
}

impl AdmissionClass {
    fn weight(self) -> u32 {
        match self {
            Self::Compensation => 8,
            Self::Interactive => 8,
            Self::Ultra => 4,
            Self::Graph => 2,
            Self::Swarm => 1,
        }
    }

    fn ordered() -> [Self; 5] {
        [
            Self::Compensation,
            Self::Interactive,
            Self::Ultra,
            Self::Graph,
            Self::Swarm,
        ]
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct AdmissionRequestId {
    pub run_id: String,
    pub node_id: String,
    pub attempt: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionRequest {
    pub id: AdmissionRequestId,
    pub class: AdmissionClass,
    pub claims: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionTicket {
    pub id: String,
    pub request_id: AdmissionRequestId,
    claims: BTreeMap<String, u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AdmissionResult {
    Acquired(AdmissionTicket),
    Queued,
    QueueFull,
    Impossible { resource: String },
}

#[derive(Debug)]
pub struct HostAdmissionController {
    capacities: BTreeMap<String, u32>,
    interactive_reserve: BTreeMap<String, u32>,
    used: BTreeMap<String, u32>,
    queues: BTreeMap<AdmissionClass, VecDeque<AdmissionRequest>>,
    queued_ids: BTreeSet<AdmissionRequestId>,
    admitted: BTreeMap<AdmissionRequestId, AdmissionTicket>,
    deficits: BTreeMap<AdmissionClass, u32>,
    cursor: usize,
    max_queue: usize,
}

impl HostAdmissionController {
    pub fn new(
        capacities: BTreeMap<String, u32>,
        interactive_reserve: BTreeMap<String, u32>,
        max_queue: usize,
    ) -> Self {
        Self {
            capacities,
            interactive_reserve,
            used: BTreeMap::new(),
            queues: AdmissionClass::ordered()
                .into_iter()
                .map(|class| (class, VecDeque::new()))
                .collect(),
            queued_ids: BTreeSet::new(),
            admitted: BTreeMap::new(),
            deficits: BTreeMap::new(),
            cursor: 0,
            max_queue: max_queue.max(1),
        }
    }

    pub fn ensure_capacity(&mut self, resource: impl Into<String>, capacity: u32) {
        let resource = resource.into();
        let entry = self.capacities.entry(resource).or_default();
        *entry = (*entry).max(capacity);
    }

    pub fn set_interactive_reserve(&mut self, resource: impl Into<String>, reserve: u32) {
        self.interactive_reserve.insert(resource.into(), reserve);
    }

    pub fn submit(&mut self, request: AdmissionRequest) -> AdmissionResult {
        if let Some(ticket) = self.admitted.remove(&request.id) {
            return AdmissionResult::Acquired(ticket);
        }
        for (resource, amount) in &request.claims {
            if *amount > self.capacities.get(resource).copied().unwrap_or(0) {
                return AdmissionResult::Impossible {
                    resource: resource.clone(),
                };
            }
        }
        if !self.queued_ids.contains(&request.id) {
            if self.queued_ids.len() >= self.max_queue {
                return AdmissionResult::QueueFull;
            }
            self.queued_ids.insert(request.id.clone());
            self.queues
                .entry(request.class)
                .or_default()
                .push_back(request.clone());
        }
        self.schedule();
        self.admitted
            .remove(&request.id)
            .map(AdmissionResult::Acquired)
            .unwrap_or(AdmissionResult::Queued)
    }

    pub fn release(&mut self, ticket: AdmissionTicket) {
        for (resource, amount) in ticket.claims {
            let used = self.used.entry(resource).or_default();
            *used = used.saturating_sub(amount);
        }
        self.schedule();
    }

    pub fn cancel(&mut self, request_id: &AdmissionRequestId) {
        self.queued_ids.remove(request_id);
        self.admitted
            .remove(request_id)
            .map(|ticket| self.release(ticket));
        for queue in self.queues.values_mut() {
            queue.retain(|request| &request.id != request_id);
        }
    }

    pub fn used(&self, resource: &str) -> u32 {
        self.used.get(resource).copied().unwrap_or(0)
    }

    fn schedule(&mut self) {
        let classes = AdmissionClass::ordered();
        let mut made_progress = true;
        while made_progress {
            made_progress = false;
            for _ in 0..classes.len() {
                let class = classes[self.cursor % classes.len()];
                self.cursor = (self.cursor + 1) % classes.len();
                *self.deficits.entry(class).or_default() += class.weight();
                if self.deficits.get(&class).copied().unwrap_or(0) == 0 {
                    continue;
                }
                let Some(index) = self.first_fitting_index(class) else {
                    continue;
                };
                let request = self
                    .queues
                    .get_mut(&class)
                    .and_then(|queue| queue.remove(index))
                    .expect("fitting request exists");
                self.queued_ids.remove(&request.id);
                *self.deficits.entry(class).or_default() -= 1;
                for (resource, amount) in &request.claims {
                    *self.used.entry(resource.clone()).or_default() += *amount;
                }
                self.admitted.insert(
                    request.id.clone(),
                    AdmissionTicket {
                        id: Uuid::new_v4().to_string(),
                        request_id: request.id,
                        claims: request.claims,
                    },
                );
                made_progress = true;
            }
        }
    }

    fn first_fitting_index(&self, class: AdmissionClass) -> Option<usize> {
        self.queues.get(&class)?.iter().position(|request| {
            request.claims.iter().all(|(resource, amount)| {
                let capacity = self.capacities.get(resource).copied().unwrap_or(0);
                let reserve = if matches!(
                    class,
                    AdmissionClass::Interactive | AdmissionClass::Compensation
                ) {
                    0
                } else {
                    self.interactive_reserve.get(resource).copied().unwrap_or(0)
                };
                self.used(resource).saturating_add(*amount) <= capacity.saturating_sub(reserve)
            })
        })
    }
}

pub fn global_admission_controller() -> &'static Mutex<HostAdmissionController> {
    static CONTROLLER: OnceLock<Mutex<HostAdmissionController>> = OnceLock::new();
    CONTROLLER.get_or_init(|| {
        Mutex::new(HostAdmissionController::new(
            BTreeMap::from([
                ("model-calls".to_string(), 101),
                ("local-execution".to_string(), 33),
                ("unisolated-writers".to_string(), 1),
            ]),
            BTreeMap::from([
                ("model-calls".to_string(), 1),
                ("local-execution".to_string(), 1),
            ]),
            10_000,
        ))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(
        run: &str,
        node: &str,
        class: AdmissionClass,
        claims: &[(&str, u32)],
    ) -> AdmissionRequest {
        AdmissionRequest {
            id: AdmissionRequestId {
                run_id: run.to_string(),
                node_id: node.to_string(),
                attempt: 1,
            },
            class,
            claims: claims
                .iter()
                .map(|(key, amount)| ((*key).to_string(), *amount))
                .collect(),
        }
    }

    #[test]
    fn interactive_reserve_survives_full_swarm_pressure() {
        let mut controller = HostAdmissionController::new(
            BTreeMap::from([("model-calls".to_string(), 3)]),
            BTreeMap::from([("model-calls".to_string(), 1)]),
            20,
        );
        let first = controller.submit(request(
            "swarm",
            "a",
            AdmissionClass::Swarm,
            &[("model-calls", 1)],
        ));
        let second = controller.submit(request(
            "swarm",
            "b",
            AdmissionClass::Swarm,
            &[("model-calls", 1)],
        ));
        assert!(matches!(first, AdmissionResult::Acquired(_)));
        assert!(matches!(second, AdmissionResult::Acquired(_)));
        assert!(matches!(
            controller.submit(request(
                "swarm",
                "c",
                AdmissionClass::Swarm,
                &[("model-calls", 1)]
            )),
            AdmissionResult::Queued
        ));
        assert!(matches!(
            controller.submit(request(
                "root",
                "prompt",
                AdmissionClass::Interactive,
                &[("model-calls", 1)]
            )),
            AdmissionResult::Acquired(_)
        ));
    }

    #[test]
    fn atomic_multi_resource_and_head_of_line_bypass_make_progress() {
        let mut controller = HostAdmissionController::new(
            BTreeMap::from([("model-calls".to_string(), 2), ("cargo".to_string(), 1)]),
            BTreeMap::new(),
            20,
        );
        let cargo = match controller.submit(request(
            "one",
            "cargo",
            AdmissionClass::Graph,
            &[("cargo", 1)],
        )) {
            AdmissionResult::Acquired(ticket) => ticket,
            other => panic!("expected ticket, got {other:?}"),
        };
        assert!(matches!(
            controller.submit(request(
                "two",
                "large",
                AdmissionClass::Graph,
                &[("model-calls", 2), ("cargo", 1)]
            )),
            AdmissionResult::Queued
        ));
        assert!(matches!(
            controller.submit(request(
                "three",
                "small",
                AdmissionClass::Graph,
                &[("model-calls", 1)]
            )),
            AdmissionResult::Acquired(_)
        ));
        controller.release(cargo);
    }
}
