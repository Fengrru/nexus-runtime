use crate::{CapabilityMode, SchedulerTask};
use nexus_core::TaskId;
use std::collections::BTreeMap;

#[cfg(feature = "docker")]
use bollard::container::{
    Config as DockerConfig, CreateContainerOptions, RemoveContainerOptions,
    StartContainerOptions,
};
use bollard::models::HostConfig;
use bollard::Docker;

pub struct DockerScheduler {
    ready_queue: Vec<SchedulerTask>,
    lock_table: BTreeMap<String, TaskId>,
    max_concurrency: usize,
    active_workers: BTreeMap<TaskId, String>,
    #[allow(dead_code)]
    docker_socket: String,
    worker_image: String,
    #[cfg(feature = "docker")]
    client: Option<Docker>,
}

impl DockerScheduler {
    pub fn new(max_concurrency: usize) -> Self {
        Self {
            ready_queue: Vec::new(),
            lock_table: BTreeMap::new(),
            max_concurrency,
            active_workers: BTreeMap::new(),
            docker_socket: "unix:///var/run/docker.sock".into(),
            worker_image: "nexus/worker:v1".into(),
            #[cfg(feature = "docker")]
            client: Docker::connect_with_local_defaults().ok(),
        }
    }

    pub fn with_image(mut self, image: &str) -> Self {
        self.worker_image = image.to_string();
        self
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

                let container_name = format!("nexus-worker-{}", hex::encode(&task.task_id.0[..8]));
                self.active_workers.insert(task.task_id, container_name);
                dispatched.push(task.task_id);
            } else {
                self.ready_queue.insert(0, task);
                break;
            }
        }
        dispatched
    }

    pub async fn dispatch_and_start(
        &mut self,
        capabilities_for: &dyn Fn(TaskId) -> Vec<String>,
    ) -> Vec<(TaskId, String)> {
        let dispatched = self.tick();
        let mut started = Vec::new();

        for &task_id in &dispatched {
            let caps = capabilities_for(task_id);
            if let Ok(container_name) = self.start_container(task_id, &caps).await {
                started.push((task_id, container_name));
            } else {
                self.release_task(task_id);
            }
        }

        started
    }

    #[cfg(feature = "docker")]
    pub async fn start_container(
        &self,
        task_id: TaskId,
        capabilities: &[String],
    ) -> Result<String, String> {
        let client = self.client.as_ref().ok_or("Docker client not connected")?;
        let container_name = format!("nexus-worker-{}", hex::encode(&task_id.0[..8]));

        let env_vars: Vec<String> = capabilities
            .iter()
            .map(|c| format!("NEXUS_CAPABILITY={}", c))
            .chain(std::iter::once(format!(
                "NEXUS_TASK_ID={}",
                hex::encode(task_id.0)
            )))
            .collect();

        let config = DockerConfig {
            image: Some(self.worker_image.clone()),
            env: Some(env_vars),
            host_config: Some(HostConfig {
                network_mode: Some("none".into()),
                readonly_rootfs: Some(true),
                auto_remove: Some(true),
                ..Default::default()
            }),
            ..Default::default()
        };

        let options = CreateContainerOptions {
            name: container_name.clone(),
            platform: None,
        };

        client
            .create_container(Some(options), config)
            .await
            .map_err(|e| format!("create container: {}", e))?;

        client
            .start_container(&container_name, None::<StartContainerOptions<String>>)
            .await
            .map_err(|e| format!("start container: {}", e))?;

        Ok(container_name)
    }

    #[cfg(not(feature = "docker"))]
    pub async fn start_container(
        &self,
        task_id: TaskId,
        _capabilities: &[String],
    ) -> Result<String, String> {
        Ok(format!("stub-container-{}", hex::encode(&task_id.0[..8])))
    }

    #[cfg(feature = "docker")]
    pub async fn stop_container(&self, task_id: TaskId) -> Result<(), String> {
        let client = self.client.as_ref().ok_or("Docker client not connected")?;
        let container_name = format!("nexus-worker-{}", hex::encode(&task_id.0[..8]));

        let options = RemoveContainerOptions {
            force: true,
            ..Default::default()
        };

        client
            .remove_container(&container_name, Some(options))
            .await
            .map_err(|e| format!("remove container: {}", e))?;

        Ok(())
    }

    #[cfg(not(feature = "docker"))]
    pub async fn stop_container(&self, _task_id: TaskId) -> Result<(), String> {
        Ok(())
    }

    #[cfg(feature = "docker")]
    pub async fn health_check(&self) -> Result<(), String> {
        if let Some(client) = &self.client {
            client
                .ping()
                .await
                .map_err(|e| format!("docker health check: {}", e))?;
            Ok(())
        } else {
            Err("Docker client not connected".into())
        }
    }

    #[cfg(not(feature = "docker"))]
    pub async fn health_check(&self) -> Result<(), String> {
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

    pub fn generate_docker_run_command(
        worker_image: &str,
        task_id: TaskId,
        capabilities: &[String],
    ) -> String {
        let caps_str = capabilities.join(",");
        format!(
            "docker run --rm --network none --read-only \
             --name nexus-worker-{} \
             -e NEXUS_WORKER_CAPABILITIES='{}' \
             {}",
            hex::encode(&task_id.0[..8]),
            caps_str,
            worker_image
        )
    }

    pub fn pending_count(&self) -> usize {
        self.ready_queue.len()
    }

    pub fn active_count(&self) -> usize {
        self.active_workers.len()
    }

    pub fn worker_statuses(&self) -> Vec<(TaskId, String)> {
        self.active_workers
            .iter()
            .map(|(tid, name)| (*tid, name.clone()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SchedulerTask;

    #[test]
    fn test_docker_scheduler_dispatches_tasks() {
        let mut sched = DockerScheduler::new(3);
        let t1 = TaskId::from_bytes([1u8; 16]);
        let t2 = TaskId::from_bytes([2u8; 16]);

        sched.enqueue(SchedulerTask {
            task_id: t1,
            required_capabilities: vec![],
            priority: 1,
        });
        sched.enqueue(SchedulerTask {
            task_id: t2,
            required_capabilities: vec![],
            priority: 1,
        });

        let dispatched = sched.tick();
        assert_eq!(dispatched.len(), 2);
        assert_eq!(sched.active_count(), 2);
    }

    #[test]
    fn test_docker_run_command_format() {
        let tid = TaskId::from_bytes([1u8; 16]);
        let cmd = DockerScheduler::generate_docker_run_command(
            "nexus/python-worker:v1.0",
            tid,
            &["fs:read:/src".into(), "fs:write:/src".into()],
        );
        assert!(cmd.contains("docker run"));
        assert!(cmd.contains("--network none"));
        assert!(cmd.contains("--read-only"));
        assert!(cmd.contains("nexus/python-worker:v1.0"));
    }

    #[test]
    fn test_docker_scheduler_releases_locks() {
        let mut sched = DockerScheduler::new(3);
        let t1 = TaskId::from_bytes([1u8; 16]);

        sched.enqueue(SchedulerTask {
            task_id: t1,
            required_capabilities: vec![crate::RequiredCapability {
                resource: "/tmp/lock1".into(),
                mode: CapabilityMode::Exclusive,
            }],
            priority: 1,
        });

        let dispatched = sched.tick();
        assert_eq!(dispatched.len(), 1);
        assert_eq!(sched.active_count(), 1);

        sched.release_task(t1);
        assert_eq!(sched.active_count(), 0);
    }

    #[tokio::test]
    async fn test_docker_stub_start_stop() {
        let sched = DockerScheduler::new(3);
        let tid = TaskId::from_bytes([1u8; 16]);

        let name = sched.start_container(tid, &[]).await.unwrap();
        assert!(name.contains("stub-container") || name.contains("nexus-worker"));

        sched.stop_container(tid).await.unwrap();
    }

    #[tokio::test]
    async fn test_docker_health_check_stub() {
        let sched = DockerScheduler::new(3);
        assert!(sched.health_check().await.is_ok());
    }
}
