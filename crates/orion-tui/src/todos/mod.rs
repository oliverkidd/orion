//! PROJECT TODOS (`⌘I`): a project's own todo list, kept on this machine
//! only (`store`), in groups that nest and fold, each item with Linear's
//! priority and sorted by it. Ticking an item strikes it through for the
//! rest of the day; from the next day on it is in the LOG under the day
//! it was done, and whatever was not done is simply still there. A list
//! pasted in as indented Markdown becomes groups and items (`import`).
//! The modal itself is `view`.
//!
//! Days are local, never UTC: a tick at 11pm is that day's. "Today" is
//! read off the clock at every draw ([`today`]), so the list rolls over
//! by itself after midnight with no day stored anywhere.

pub mod import;
pub mod store;
pub mod view;

use chrono::{DateTime, Local, NaiveDate};

pub use store::{Group, Item, TodoFile};
pub(crate) use view::{draw, handle_key, handle_mouse, open, paste, reopen, TodoView};

/// The time now, local — or the one a test set ([`with_now`]).
pub fn now() -> DateTime<Local> {
    #[cfg(test)]
    if let Some(at) = NOW.with(|n| *n.borrow()) {
        return at;
    }
    Local::now()
}

/// Today's local date: what "done today" and an item's age count from.
pub fn today() -> NaiveDate {
    now().date_naive()
}

#[cfg(test)]
thread_local! {
    static NOW: std::cell::RefCell<Option<DateTime<Local>>> =
        const { std::cell::RefCell::new(None) };
}

/// Run `f` with the clock stopped at `at`.
#[cfg(test)]
pub fn with_now<T>(at: DateTime<Local>, f: impl FnOnce() -> T) -> T {
    NOW.with(|slot| {
        let prev = slot.replace(Some(at));
        let out = f();
        slot.replace(prev);
        out
    })
}

/// The todo an agent launch was sent at: which list, which item, and the
/// URL of the Linear issue it is linked to — the session's context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoRef {
    pub repo_path: std::path::PathBuf,
    pub item: u64,
    pub issue_url: Option<String>,
}

/// The DAEMON made the session a launch from the TODOS MODAL asked for:
/// the item remembers it — its chip, and `Enter`'s way back to it.
pub(crate) fn agent_started(app: &mut crate::app::App, todo: TodoRef, agent: &orion_core::AgentId) {
    let Some(file) = app.todos.get_mut(&todo.repo_path) else {
        return;
    };
    if let Some(item) = file.item_mut(todo.item) {
        item.agent = Some(agent.0.clone());
        store::save(file);
        app.dirty = true;
    }
}

/// How deep groups nest before the walks stop: a hand-edited file with a
/// group under itself must not hang the draw.
const MAX_DEPTH: usize = 32;

impl Item {
    /// The local day it was ticked, if it was.
    pub fn done_on(&self) -> Option<NaiveDate> {
        self.done.map(|at| at.date_naive())
    }

    /// Ticked on `today`: struck through, at the bottom of its group.
    pub fn done_today(&self, today: NaiveDate) -> bool {
        self.done_on() == Some(today)
    }

    /// On the Today tab: still open, or ticked today.
    pub fn on_today(&self, today: NaiveDate) -> bool {
        self.done.is_none() || self.done_today(today)
    }

    /// Whole days an open item has carried over: 0 for one written today.
    pub fn age(&self, today: NaiveDate) -> i64 {
        (today - self.created).num_days().max(0)
    }
}

/// How many priorities there are: Linear's four and none.
pub const PRIORITIES: usize = crate::linear::PRIORITY_ORDER.len();

/// What a group header totals, nested groups included: the open items at
/// each priority — by [`crate::linear::priority_rank`], urgent first —
/// and the ones ticked today.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub open: [usize; PRIORITIES],
    pub done_today: usize,
}

impl Counts {
    pub fn open_total(&self) -> usize {
        self.open.iter().sum()
    }
}

impl TodoFile {
    /// An id no group or item has — or ever had: the counter only goes
    /// up, and starts past every id in a file written before it was kept.
    pub fn take_id(&mut self) -> u64 {
        let groups = self.groups.iter().map(|g| g.id);
        let items = self.items.iter().map(|i| i.id);
        let past = groups.chain(items).max().map_or(1, |id| id + 1);
        let id = self.next_id.max(past);
        self.next_id = id + 1;
        id
    }

    pub fn group(&self, id: u64) -> Option<&Group> {
        self.groups.iter().find(|g| g.id == id)
    }

    pub fn group_mut(&mut self, id: u64) -> Option<&mut Group> {
        self.groups.iter_mut().find(|g| g.id == id)
    }

    pub fn item(&self, id: u64) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }

    pub fn item_mut(&mut self, id: u64) -> Option<&mut Item> {
        self.items.iter_mut().find(|i| i.id == id)
    }

    /// The groups directly under `parent` (`None`: the top), in order.
    pub fn subgroups(&self, parent: Option<u64>) -> impl Iterator<Item = &Group> {
        self.groups.iter().filter(move |g| g.parent == parent)
    }

    /// A group's items on the Today tab, in the order they are drawn: the
    /// open ones by priority, urgent first, each priority in the order
    /// they were written down; then the ones ticked today, in the order
    /// they were ticked.
    pub fn today_items(&self, group: u64, today: NaiveDate) -> Vec<&Item> {
        let mut open: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.group == group && i.done.is_none())
            .collect();
        // Stable: equal priorities keep the order they were made in.
        open.sort_by_key(|i| crate::linear::priority_rank(i.priority));
        let mut done: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.group == group && i.done_today(today))
            .collect();
        done.sort_by_key(|i| i.done);
        open.extend(done);
        open
    }

    /// `group`'s header counts, every group under it added in.
    pub fn counts(&self, group: u64, today: NaiveDate) -> Counts {
        let mut counts = Counts::default();
        self.add_counts(group, today, &mut counts, 0);
        counts
    }

    fn add_counts(&self, group: u64, today: NaiveDate, counts: &mut Counts, depth: usize) {
        if depth > MAX_DEPTH {
            return;
        }
        for item in self.items.iter().filter(|i| i.group == group) {
            if item.done.is_none() {
                counts.open[crate::linear::priority_rank(item.priority).min(PRIORITIES - 1)] += 1;
            } else if item.done_today(today) {
                counts.done_today += 1;
            }
        }
        for sub in self.subgroups(Some(group)) {
            self.add_counts(sub.id, today, counts, depth + 1);
        }
    }

    /// Every open item, the Today tab's badge.
    pub fn open_count(&self) -> usize {
        self.items.iter().filter(|i| i.done.is_none()).count()
    }

    /// The names from the top down to `group`: `["MCP fixes", "storyline
    /// prompt"]`.
    pub fn path(&self, group: u64) -> Vec<&str> {
        let mut names = Vec::new();
        let mut at = self.group(group);
        while let Some(g) = at {
            if names.len() > MAX_DEPTH {
                break;
            }
            names.push(g.name.as_str());
            at = g.parent.and_then(|p| self.group(p));
        }
        names.reverse();
        names
    }

    /// [`TodoFile::path`] as a line reads it: `MCP fixes › storyline prompt`.
    pub fn path_label(&self, group: u64) -> String {
        self.path(group).join(" › ")
    }

    /// `group` and every group under it, outermost first.
    pub fn subtree(&self, group: u64) -> Vec<u64> {
        let mut out = vec![group];
        let mut i = 0;
        while i < out.len() && out.len() <= self.groups.len() {
            let id = out[i];
            out.extend(self.subgroups(Some(id)).map(|g| g.id));
            i += 1;
        }
        out
    }

    /// How many items `group` holds, nested ones and ticked ones included:
    /// what deleting it would take with it.
    pub fn size(&self, group: u64) -> usize {
        let groups = self.subtree(group);
        self.items
            .iter()
            .filter(|i| groups.contains(&i.group))
            .count()
    }

    /// The LOG: every day before `today` that something was ticked on,
    /// the most recent first, each with its items, the latest tick first.
    pub fn log_days(&self, today: NaiveDate) -> Vec<(NaiveDate, Vec<&Item>)> {
        let mut done: Vec<&Item> = self
            .items
            .iter()
            .filter(|i| i.done_on().is_some_and(|d| d < today))
            .collect();
        done.sort_by_key(|i| std::cmp::Reverse(i.done));
        let mut days: Vec<(NaiveDate, Vec<&Item>)> = Vec::new();
        for item in done {
            let day = item.done_on().unwrap_or(today);
            match days.last_mut() {
                Some((d, items)) if *d == day => items.push(item),
                _ => days.push((day, vec![item])),
            }
        }
        days
    }

    /// A new group named `name` under `parent`, after its siblings.
    pub fn add_group(&mut self, parent: Option<u64>, name: &str) -> u64 {
        let id = self.take_id();
        self.groups.push(Group {
            id,
            parent,
            name: name.to_string(),
            collapsed: false,
        });
        id
    }

    /// A new open item in `group`, written down `today`.
    pub fn add_item(&mut self, group: u64, text: &str, today: NaiveDate) -> u64 {
        let id = self.take_id();
        self.items.push(Item {
            id,
            group,
            text: text.to_string(),
            priority: 0,
            created: today,
            done: None,
            linear: None,
            agent: None,
            linear_seen: None,
        });
        id
    }

    /// Tick `item` at `now`, or untick it. True when it is now done.
    pub fn toggle(&mut self, item: u64, now: DateTime<Local>) -> bool {
        let Some(item) = self.item_mut(item) else {
            return false;
        };
        item.done = match item.done {
            Some(_) => None,
            None => Some(now),
        };
        item.done.is_some()
    }

    pub fn delete_item(&mut self, item: u64) {
        self.items.retain(|i| i.id != item);
    }

    /// `group`, every group under it and all their items.
    pub fn delete_group(&mut self, group: u64) {
        let gone = self.subtree(group);
        self.items.retain(|i| !gone.contains(&i.group));
        self.groups.retain(|g| !gone.contains(&g.id));
    }

    /// The top-level group named [`import::INBOX`], made when missing:
    /// where an item goes with no group to go in.
    pub fn inbox(&mut self) -> u64 {
        let inbox = self
            .subgroups(None)
            .find(|g| g.name == import::INBOX)
            .map(|g| g.id);
        match inbox {
            Some(id) => id,
            None => self.add_group(None, import::INBOX),
        }
    }

    /// A pasted list's groups and items added in, open and with no
    /// priority — merged into a group of the same name at the same level
    /// rather than beside it, an item its group already has left out, so
    /// the same list pasted twice adds nothing. How many items came in.
    pub fn import(&mut self, nodes: &[import::Node], today: NaiveDate) -> usize {
        self.import_under(None, nodes, today, 0)
    }

    fn import_under(
        &mut self,
        parent: Option<u64>,
        nodes: &[import::Node],
        today: NaiveDate,
        depth: usize,
    ) -> usize {
        let mut added = 0;
        for node in nodes {
            match node {
                import::Node::Item(text) => {
                    let group = match parent {
                        Some(group) => group,
                        None => self.inbox(),
                    };
                    if self
                        .items
                        .iter()
                        .any(|i| i.group == group && i.text == *text)
                    {
                        continue;
                    }
                    self.add_item(group, text, today);
                    added += 1;
                }
                import::Node::Group { name, children } if depth < MAX_DEPTH => {
                    let existing = self
                        .subgroups(parent)
                        .find(|g| g.name == *name)
                        .map(|g| g.id);
                    let group = existing.unwrap_or_else(|| self.add_group(parent, name));
                    added += self.import_under(Some(group), children, today, depth + 1);
                }
                import::Node::Group { .. } => {}
            }
        }
        added
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use std::path::Path;

    fn day(d: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(2026, 10, d).unwrap()
    }

    fn at(d: u32, h: u32) -> DateTime<Local> {
        Local.with_ymd_and_hms(2026, 10, d, h, 0, 0).unwrap()
    }

    fn texts(items: &[&Item]) -> Vec<String> {
        items.iter().map(|i| i.text.clone()).collect()
    }

    /// Open items by priority, urgent first and none last, each priority
    /// in the order written; ticked-today ones after, in tick order.
    #[test]
    fn a_group_sorts_by_priority_then_ticks_last() {
        let mut file = TodoFile::new(Path::new("/r"));
        let g = file.add_group(None, "G");
        for (text, priority) in [
            ("none", 0),
            ("low", 4),
            ("urgent", 1),
            ("high", 2),
            ("high2", 2),
        ] {
            let id = file.add_item(g, text, day(6));
            file.item_mut(id).unwrap().priority = priority;
        }
        let a = file.add_item(g, "ticked second", day(6));
        let b = file.add_item(g, "ticked first", day(6));
        file.toggle(b, at(6, 9));
        file.toggle(a, at(6, 10));
        assert_eq!(
            texts(&file.today_items(g, day(6))),
            [
                "urgent",
                "high",
                "high2",
                "low",
                "none",
                "ticked first",
                "ticked second"
            ]
        );
    }

    /// A header counts its nested groups' items too: the open ones at
    /// each priority, and the ones ticked today — not those ticked before.
    #[test]
    fn counts_take_in_nested_groups() {
        let mut file = TodoFile::new(Path::new("/r"));
        let top = file.add_group(None, "MCP fixes");
        let sub = file.add_group(Some(top), "storyline prompt");
        let deeper = file.add_group(Some(sub), "deeper");
        let a = file.add_item(top, "a", day(1));
        file.item_mut(a).unwrap().priority = 1;
        let b = file.add_item(sub, "b", day(1));
        file.item_mut(b).unwrap().priority = 2;
        file.add_item(deeper, "c", day(1));
        let today = file.add_item(sub, "done today", day(1));
        file.toggle(today, at(6, 23));
        let earlier = file.add_item(sub, "done yesterday", day(1));
        file.toggle(earlier, at(5, 12));
        let counts = file.counts(top, day(6));
        assert_eq!(counts.open, [1, 1, 0, 0, 1]);
        assert_eq!(counts.open_total(), 3);
        assert_eq!(counts.done_today, 1);
        assert_eq!(file.counts(deeper, day(6)).open_total(), 1);
        assert_eq!(
            file.path_label(deeper),
            "MCP fixes › storyline prompt › deeper"
        );
    }

    /// Done today stays on Today, struck through; from the next day it
    /// is in the log under the day it was done, and what is still open
    /// carries over, a day older.
    #[test]
    fn the_day_rolls_over_with_the_clock() {
        let mut file = TodoFile::new(Path::new("/r"));
        let g = file.add_group(None, "G");
        let done = file.add_item(g, "ship it", day(6));
        let open = file.add_item(g, "still open", day(6));
        file.toggle(done, at(6, 23));
        let item = |file: &TodoFile, id| file.item(id).unwrap().clone();
        assert!(item(&file, done).on_today(day(6)));
        assert!(file.log_days(day(6)).is_empty());
        assert_eq!(file.counts(g, day(6)).done_today, 1);

        assert!(!item(&file, done).on_today(day(7)));
        assert_eq!(texts(&file.today_items(g, day(7))), ["still open"]);
        let log = file.log_days(day(7));
        assert_eq!(log.len(), 1);
        assert_eq!(log[0].0, day(6));
        assert_eq!(texts(&log[0].1), ["ship it"]);
        assert_eq!(item(&file, open).age(day(7)), 1);
        assert_eq!(file.counts(g, day(7)).done_today, 0);

        // Unticked from the log, it is open again.
        file.toggle(done, at(7, 9));
        assert_eq!(
            texts(&file.today_items(g, day(7))),
            ["ship it", "still open"]
        );
    }

    /// The log reads newest day first.
    #[test]
    fn the_log_is_most_recent_first() {
        let mut file = TodoFile::new(Path::new("/r"));
        let g = file.add_group(None, "G");
        for (text, d) in [("mon", 5), ("sat", 3), ("mon2", 5)] {
            let id = file.add_item(g, text, day(1));
            file.toggle(id, at(d, 12));
        }
        let log = file.log_days(day(6));
        let days: Vec<NaiveDate> = log.iter().map(|(d, _)| *d).collect();
        assert_eq!(days, [day(5), day(3)]);
        assert_eq!(log[0].1.len(), 2);
    }

    /// Importing twice merges into the groups already there.
    #[test]
    fn an_import_merges_into_groups_of_the_same_name() {
        let mut file = TodoFile::new(Path::new("/r"));
        let nodes = import::parse(import::tests::FIXTURE);
        assert_eq!(file.import(&nodes, day(6)), 29);
        let tops: Vec<&str> = file.subgroups(None).map(|g| g.name.as_str()).collect();
        assert_eq!(
            tops,
            ["Emails", "link sharing", "UI", "MCP fixes", "Side quests"]
        );
        let groups = file.groups.len();
        assert_eq!(
            file.import(&import::parse("- Emails\n    - one more\n- solo\n"), day(6)),
            2
        );
        assert_eq!(file.groups.len(), groups + 1, "Emails merged, Inbox made");
        let emails = file
            .subgroups(None)
            .find(|g| g.name == "Emails")
            .unwrap()
            .id;
        assert_eq!(file.today_items(emails, day(6)).len(), 6);
        assert!(file
            .items
            .iter()
            .all(|i| i.priority == 0 && i.done.is_none()));
    }

    /// A deleted item's id is never handed out again — not even across a
    /// save — and a file from before the counter starts past its ids.
    #[test]
    fn ids_are_never_reused() {
        let mut file = TodoFile::new(Path::new("/r"));
        let g = file.add_group(None, "G");
        let a = file.add_item(g, "a", day(6));
        let b = file.add_item(g, "b", day(6));
        file.delete_item(b);
        let back: TodoFile = serde_json::from_str(&serde_json::to_string(&file).unwrap()).unwrap();
        let mut file = back;
        let c = file.add_item(g, "c", day(6));
        assert!(c > b && c != a, "{a} {b} {c}");
        let mut old = TodoFile::new(Path::new("/r"));
        old.groups.push(Group {
            id: 9,
            parent: None,
            name: "old".into(),
            collapsed: false,
        });
        assert_eq!(old.take_id(), 10);
    }

    /// The same list pasted again adds nothing.
    #[test]
    fn a_list_pasted_twice_adds_nothing_the_second_time() {
        let mut file = TodoFile::new(Path::new("/r"));
        let nodes = import::parse(import::tests::FIXTURE);
        assert_eq!(file.import(&nodes, day(6)), 29);
        assert_eq!(file.import(&nodes, day(6)), 0);
        assert_eq!(file.items.len(), 29);
    }

    /// Deleting a group takes everything under it.
    #[test]
    fn deleting_a_group_takes_its_subtree() {
        let mut file = TodoFile::new(Path::new("/r"));
        let top = file.add_group(None, "A");
        let sub = file.add_group(Some(top), "B");
        let keep = file.add_group(None, "C");
        file.add_item(sub, "x", day(6));
        file.add_item(keep, "y", day(6));
        assert_eq!(file.size(top), 1);
        file.delete_group(top);
        assert_eq!(file.groups.len(), 1);
        assert_eq!(file.items.len(), 1);
        assert_eq!(file.items[0].text, "y");
    }
}
