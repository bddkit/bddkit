//! What a run counts as it goes, for the payload: leaf steps and their time
//! per step group, and outline rows. One atomic pair per group; the group list
//! is fixed at startup, so the map is never written to and needs no lock
//! (a lock held across a step would serialise the workers).

use super::OTHER;
use super::payload::Usage;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

#[derive(Debug, Default)]
pub struct Counters {
    examples: AtomicU64,
    usage: BTreeMap<String, (AtomicU64, AtomicU64)>,
}

impl Counters {
    /// `groups`: the builtin groups and the official plugin groups. `other`
    /// is always there.
    pub fn new<'a>(groups: impl IntoIterator<Item = &'a str>) -> Self {
        let usage = groups
            .into_iter()
            .chain([OTHER])
            .map(|group| (group.to_string(), Default::default()))
            .collect();
        Counters {
            examples: AtomicU64::new(0),
            usage,
        }
    }

    /// One scenario that is a row of an outline.
    pub fn example(&self) {
        self.examples.fetch_add(1, Ordering::Relaxed);
    }

    /// One leaf step that ran: a macro call and an `I include` are not leaves,
    /// the steps they run are. A group nobody declared counts as `other`.
    pub fn leaf(&self, group: &str, elapsed: Duration) {
        let (steps, micros) = self
            .usage
            .get(group)
            .or_else(|| self.usage.get(OTHER))
            .expect("`other` is always declared");
        steps.fetch_add(1, Ordering::Relaxed);
        micros.fetch_add(
            u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
    }

    pub fn examples(&self) -> u64 {
        self.examples.load(Ordering::Relaxed)
    }

    /// Only groups that ran a step. Time is summed in microseconds and reported
    /// in milliseconds, so many fast steps do not round to nothing one by one.
    pub fn usage(&self) -> BTreeMap<String, Usage> {
        self.usage
            .iter()
            .filter_map(|(group, (steps, micros))| {
                let steps = steps.load(Ordering::Relaxed);
                (steps > 0).then(|| {
                    (
                        group.clone(),
                        Usage {
                            steps,
                            ms: micros.load(Ordering::Relaxed) / 1000,
                        },
                    )
                })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leaves_are_summed_per_group_and_an_unknown_group_is_other() {
        let counters = Counters::new(["api", "vars"]);
        counters.leaf("api", Duration::from_micros(1_600));
        counters.leaf("api", Duration::from_micros(1_600));
        counters.leaf("some plugin", Duration::from_millis(5));
        counters.example();
        counters.example();
        let usage = counters.usage();
        assert_eq!(usage["api"], Usage { steps: 2, ms: 3 });
        assert_eq!(usage["other"], Usage { steps: 1, ms: 5 });
        assert!(
            !usage.contains_key("vars"),
            "a group that ran nothing is absent"
        );
        assert_eq!(counters.examples(), 2);
    }
}
