use crate::{CapabilityMode, SchedulerTask};
#[cfg(feature = "kube-integration")]
use k8s_openapi::api::core::v1::{
    Container, EnvVar, Pod, PodSpec, ResourceRequirements, SecurityContext,
};
/// Kubernetes scheduler using the kube crate.
/// Manages worker pods in a K8s cluster.
#[cfg(feature = "kube-integration")]
use kube::api::{Api, DeleteParams, PostParams};
use kube::Client;
use nexus_core::TaskId;
use std::collections::BTreeMap;

pub struct RealK8sScheduler {
    ready_queue: Vec<SchedulerTask>,
    lock_table: BTreeMap<String, TaskId>,
    max_concurrency: usize,
    active_workers: BTreeMap<TaskId, String>,
    #[allow(dead_code)]
    namespace: String,
    #[allow(dead_code)]
    worker_image: String,
    #[cfg(feature = "kube-integration")]
    client: Option<Client>,
}

impl RealK8sScheduler {
    #[cfg(feature = "kube-integration")]
    pub async fn new(max_concurrency: usize, namespace: String, worker_image: String) -> Self {
        let client = Client::try_default().await.ok();

        if client.is_none() {
            tracing::warn!(
                target = "nexus.scheduler.k8s",
                "Could not create K8s client; check kubeconfig or in-cluster config"
            );
        }

        Self {
            ready_queue: Vec::new(),
            lock_table: BTreeMap::new(),
            max_concurrency,
            active_workers: BTreeMap::new(),
            namespace,
            worker_image,
            client,
        }
    }

    #[cfg(not(feature = "kube"))]
    pub async fn new(max_concurrency: usize, namespace: String, worker_image: String) -> Self {
        Self {
            ready_queue: Vec::new(),
            lock_table: BTreeMap::new(),
            max_concurrency,
            active_workers: BTreeMap::new(),
            namespace,
            worker_image,
        }
    }

    pub fn enqueue(&mut self, task: SchedulerTask) {
        self.ready_queue.push(task);
    }

    pub fn tick(&mut self) -> Vec<TaskId> {
        let mut dispatched = Vec::new();
        while dispatched.len() < self.max_concurrency
            && self.active_workers.len() < self.max_concurrency
        {
            let Some(task) = self.ready_queue.pop() else {
                break;
            };
            let can_dispatch = task.required_capabilities.iter().all(|cap| match cap.mode {
                CapabilityMode::Exclusive => !self.lock_table.contains_key(&cap.resource),
                CapabilityMode::Shared => true,
            });

            if can_dispatch {
                for cap in &task.required_capabilities {
                    if cap.mode == CapabilityMode::Exclusive {
                        self.lock_table.insert(cap.resource.clone(), task.task_id);
                    }
                }
                let pod_name = format!("nexus-worker-{}", hex::encode(&task.task_id.0[..8]));
                self.active_workers.insert(task.task_id, pod_name);
                dispatched.push(task.task_id);
            } else {
                self.ready_queue.insert(0, task);
                break;
            }
        }
        dispatched
    }

    #[cfg(feature = "kube-integration")]
    pub async fn create_pod(
        &self,
        task_id: TaskId,
        capabilities: &[String],
    ) -> Result<String, String> {
        let client = self.client.as_ref().ok_or("no K8s client")?;
        let pod_name = format!("nexus-worker-{}", hex::encode(&task_id.0[..8]));

        let pod = Pod {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(pod_name.clone()),
                namespace: Some(self.namespace.clone()),
                labels: Some({
                    let mut labels = BTreeMap::new();
                    labels.insert("app".into(), "nexus-worker".into());
                    labels.insert("task-id".into(), hex::encode(task_id.0));
                    labels
                }),
                ..Default::default()
            },
            spec: Some(PodSpec {
                restart_policy: Some("Never".into()),
                service_account_name: Some("nexus-worker".into()),
                containers: vec![Container {
                    name: "worker".into(),
                    image: Some(self.worker_image.clone()),
                    env: Some(
                        capabilities
                            .iter()
                            .map(|c| EnvVar {
                                name: "NEXUS_CAPABILITY".into(),
                                value: Some(c.clone()),
                                ..Default::default()
                            })
                            .collect(),
                    ),
                    security_context: Some(SecurityContext {
                        read_only_root_filesystem: Some(true),
                        allow_privilege_escalation: Some(false),
                        run_as_non_root: Some(true),
                        ..Default::default()
                    }),
                    resources: Some(ResourceRequirements {
                        claims: None,
                        requests: Some({
                            let mut req = BTreeMap::new();
                            req.insert(
                                "cpu".into(),
                                k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                                    "250m".into(),
                                ),
                            );
                            req.insert(
                                "memory".into(),
                                k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                                    "256Mi".into(),
                                ),
                            );
                            req
                        }),
                        limits: Some({
                            let mut lim = BTreeMap::new();
                            lim.insert(
                                "cpu".into(),
                                k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                                    "500m".into(),
                                ),
                            );
                            lim.insert(
                                "memory".into(),
                                k8s_openapi::apimachinery::pkg::api::resource::Quantity(
                                    "512Mi".into(),
                                ),
                            );
                            lim
                        }),
                    }),
                    ..Default::default()
                }],
                ..Default::default()
            }),
            ..Default::default()
        };

        let api: Api<Pod> = Api::namespaced(client.clone(), &self.namespace);
        api.create(&PostParams::default(), &pod)
            .await
            .map_err(|e| format!("create pod: {}", e))?;

        tracing::info!(
            target = "nexus.scheduler.k8s",
            pod = %pod_name,
            namespace = %self.namespace,
            "Pod created"
        );

        Ok(pod_name)
    }

    #[cfg(feature = "kube-integration")]
    pub async fn delete_pod(&self, task_id: TaskId) -> Result<(), String> {
        let client = self.client.as_ref().ok_or("no K8s client")?;
        let pod_name = format!("nexus-worker-{}", hex::encode(&task_id.0[..8]));

        let api: Api<Pod> = Api::namespaced(client.clone(), &self.namespace);
        api.delete(&pod_name, &DeleteParams::default())
            .await
            .map_err(|e| format!("delete pod: {}", e))?;

        Ok(())
    }

    #[cfg(not(feature = "kube"))]
    pub async fn create_pod(
        &self,
        _task_id: TaskId,
        _capabilities: &[String],
    ) -> Result<String, String> {
        Ok("stub-pod".into())
    }

    #[cfg(not(feature = "kube"))]
    pub async fn delete_pod(&self, _task_id: TaskId) -> Result<(), String> {
        Ok(())
    }

    pub fn release_task(&mut self, task_id: TaskId) {
        let to_remove: Vec<String> = self
            .lock_table
            .iter()
            .filter(|(_, owner)| **owner == task_id)
            .map(|(resource, _)| resource.clone())
            .collect();
        for resource in to_remove {
            self.lock_table.remove(&resource);
        }
        self.active_workers.remove(&task_id);
    }

    pub fn pending_count(&self) -> usize {
        self.ready_queue.len()
    }
    pub fn active_count(&self) -> usize {
        self.active_workers.len()
    }
    pub fn is_connected(&self) -> bool {
        #[cfg(feature = "kube-integration")]
        {
            self.client.is_some()
        }
        #[cfg(not(feature = "kube"))]
        {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_k8s_scheduler_dispatches() {
        let mut sched = RealK8sScheduler::new(3, "nexus".into(), "nexus/worker:v1".into()).await;

        let t1 = TaskId::from_bytes([1u8; 16]);
        sched.enqueue(SchedulerTask {
            task_id: t1,
            required_capabilities: vec![],
            priority: 1,
        });
        let dispatched = sched.tick();
        assert_eq!(dispatched.len(), 1);
    }
}
