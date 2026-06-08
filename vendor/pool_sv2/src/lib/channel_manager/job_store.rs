use std::{collections::HashMap, fmt::Debug};

use stratum_apps::stratum_core::channels_sv2::server::jobs::{
    Job,
    job_store::JobStore,
};

const MAX_FUTURE_JOBS: usize = 2;
const MAX_PAST_JOBS: usize = 2;
const MAX_STALE_JOBS: usize = 1;

#[derive(Debug)]
pub struct BoundedJobStore<T: Job + Clone> {
    future_template_to_job_id: HashMap<u64, u32>,
    future_jobs: HashMap<u32, T>,
    future_order: Vec<(u64, u32)>,
    active_job: Option<T>,
    past_jobs: HashMap<u32, T>,
    past_order: Vec<u32>,
    stale_jobs: HashMap<u32, T>,
    stale_order: Vec<u32>,
}

impl<T: Job + Clone> BoundedJobStore<T> {
    pub fn new() -> Self {
        Self {
            future_template_to_job_id: HashMap::new(),
            future_jobs: HashMap::new(),
            future_order: Vec::new(),
            active_job: None,
            past_jobs: HashMap::new(),
            past_order: Vec::new(),
            stale_jobs: HashMap::new(),
            stale_order: Vec::new(),
        }
    }

    fn insert_past_job(&mut self, job: T) {
        let job_id = job.get_job_id();
        self.past_jobs.insert(job_id, job);
        self.past_order.retain(|id| *id != job_id);
        self.past_order.push(job_id);
        self.prune_past_jobs();
    }

    fn prune_future_jobs(&mut self) {
        while self.future_order.len() > MAX_FUTURE_JOBS {
            let (template_id, job_id) = self.future_order.remove(0);
            self.future_template_to_job_id.remove(&template_id);
            self.future_jobs.remove(&job_id);
        }
        self.future_jobs.shrink_to_fit();
        self.future_template_to_job_id.shrink_to_fit();
        self.future_order.shrink_to_fit();
    }

    fn prune_past_jobs(&mut self) {
        while self.past_order.len() > MAX_PAST_JOBS {
            let job_id = self.past_order.remove(0);
            self.past_jobs.remove(&job_id);
        }
        self.past_jobs.shrink_to_fit();
        self.past_order.shrink_to_fit();
    }

    fn prune_stale_jobs(&mut self) {
        while self.stale_order.len() > MAX_STALE_JOBS {
            let job_id = self.stale_order.remove(0);
            self.stale_jobs.remove(&job_id);
        }
        self.stale_jobs.shrink_to_fit();
        self.stale_order.shrink_to_fit();
    }
}

impl<T: Job + Clone> Default for BoundedJobStore<T> {
    fn default() -> Self {
        Self::new()
    }
}

impl<T: Job + Clone + Debug> JobStore<T> for BoundedJobStore<T> {
    fn add_future_job(&mut self, template_id: u64, new_job: T) -> u32 {
        let new_job_id = new_job.get_job_id();
        self.future_jobs.insert(new_job_id, new_job);
        self.future_template_to_job_id
            .insert(template_id, new_job_id);
        self.future_order
            .retain(|(stored_template_id, stored_job_id)| {
                *stored_template_id != template_id && *stored_job_id != new_job_id
            });
        self.future_order.push((template_id, new_job_id));
        self.prune_future_jobs();
        new_job_id
    }

    fn add_active_job(&mut self, job: T) {
        if let Some(active_job) = self.active_job.take() {
            self.insert_past_job(active_job);
        }
        self.active_job = Some(job);
    }

    fn activate_future_job(&mut self, template_id: u64, prev_hash_header_timestamp: u32) -> bool {
        let mut future_job =
            if let Some(job_id) = self.future_template_to_job_id.remove(&template_id) {
                if let Some(job) = self.future_jobs.remove(&job_id) {
                    self.future_order
                        .retain(|(stored_template_id, stored_job_id)| {
                            *stored_template_id != template_id && *stored_job_id != job_id
                        });
                    job
                } else {
                    return false;
                }
            } else {
                return false;
            };

        if let Some(active_job) = self.active_job.take() {
            self.insert_past_job(active_job);
        }

        future_job.activate(prev_hash_header_timestamp);
        self.active_job = Some(future_job);
        self.future_jobs.clear();
        self.future_template_to_job_id.clear();
        self.future_order.clear();
        self.future_jobs.shrink_to_fit();
        self.future_template_to_job_id.shrink_to_fit();
        self.future_order.shrink_to_fit();

        self.mark_past_jobs_as_stale();

        true
    }

    fn mark_past_jobs_as_stale(&mut self) {
        for job_id in self.past_order.drain(..) {
            if let Some(job) = self.past_jobs.remove(&job_id) {
                self.stale_jobs.insert(job_id, job);
                self.stale_order.retain(|stored_job_id| *stored_job_id != job_id);
                self.stale_order.push(job_id);
            }
        }
        self.past_jobs.clear();
        self.past_jobs.shrink_to_fit();
        self.past_order.shrink_to_fit();
        self.prune_stale_jobs();
    }

    fn get_future_job_id_from_template_id(&self, template_id: u64) -> Option<u32> {
        self.future_template_to_job_id.get(&template_id).cloned()
    }

    fn get_active_job(&self) -> Option<T> {
        self.active_job.clone()
    }

    fn has_future_jobs(&self) -> bool {
        !self.future_jobs.is_empty()
    }

    fn get_future_job(&self, job_id: u32) -> Option<T> {
        self.future_jobs.get(&job_id).cloned()
    }

    fn has_past_jobs(&self) -> bool {
        !self.past_jobs.is_empty()
    }

    fn get_past_job(&self, job_id: u32) -> Option<T> {
        self.past_jobs.get(&job_id).cloned()
    }

    fn has_stale_jobs(&self) -> bool {
        !self.stale_jobs.is_empty()
    }

    fn get_stale_job(&self, job_id: u32) -> Option<T> {
        self.stale_jobs.get(&job_id).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct TestJob {
        job_id: u32,
        activated_at: Option<u32>,
    }

    impl TestJob {
        fn new(job_id: u32) -> Self {
            Self {
                job_id,
                activated_at: None,
            }
        }
    }

    impl Job for TestJob {
        fn get_job_id(&self) -> u32 {
            self.job_id
        }

        fn activate(&mut self, prev_hash_header_timestamp: u32) {
            self.activated_at = Some(prev_hash_header_timestamp);
        }
    }

    #[test]
    fn future_jobs_are_capped() {
        let mut store = BoundedJobStore::new();

        store.add_future_job(10, TestJob::new(1));
        store.add_future_job(11, TestJob::new(2));
        store.add_future_job(12, TestJob::new(3));

        assert_eq!(store.get_future_job_id_from_template_id(10), None);
        assert_eq!(store.get_future_job_id_from_template_id(11), Some(2));
        assert_eq!(store.get_future_job_id_from_template_id(12), Some(3));
        assert_eq!(store.get_future_job(1), None);
    }

    #[test]
    fn past_jobs_are_capped() {
        let mut store = BoundedJobStore::new();

        store.add_active_job(TestJob::new(1));
        store.add_active_job(TestJob::new(2));
        store.add_active_job(TestJob::new(3));
        store.add_active_job(TestJob::new(4));

        assert_eq!(store.get_past_job(1), None);
        assert_eq!(store.get_past_job(2).unwrap().job_id, 2);
        assert_eq!(store.get_past_job(3).unwrap().job_id, 3);
        assert_eq!(store.get_active_job().unwrap().job_id, 4);
    }

    #[test]
    fn stale_jobs_are_capped() {
        let mut store = BoundedJobStore::new();

        store.add_active_job(TestJob::new(1));
        store.add_active_job(TestJob::new(2));
        store.mark_past_jobs_as_stale();
        store.add_active_job(TestJob::new(3));
        store.mark_past_jobs_as_stale();

        assert_eq!(store.get_stale_job(1), None);
        assert_eq!(store.get_stale_job(2).unwrap().job_id, 2);
        assert_eq!(store.get_stale_job(3), None);
    }

    #[test]
    fn active_job_remains_available_after_pruning() {
        let mut store = BoundedJobStore::new();

        store.add_active_job(TestJob::new(1));
        store.add_active_job(TestJob::new(2));
        store.add_active_job(TestJob::new(3));
        store.add_active_job(TestJob::new(4));

        assert_eq!(store.get_active_job().unwrap().job_id, 4);
    }
}
