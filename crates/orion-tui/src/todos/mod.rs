//! PROJECT TODOS (`⌘I`): a project's own todo list, kept on this machine
//! only (`store`), in groups that fold, each item with Linear's priority.
//! The order is the list's own: `⌥↑`/`⌥↓` walk an item anywhere in it —
//! into a nested group and back out — and a priority set puts an item at
//! the end of its level's run. `⌥→` groups items, `⌥←` takes them out
//! again. Ticking an item strikes it through at the bottom of its group
//! while the modal is up; once it goes, so does the item — kept in the
//! file for the DONE page (`history`), what was ticked today or this
//! week. A group with everything in it ticked is complete, and goes the
//! same way. A list pasted in as indented Markdown becomes groups and
//! items (`import`). The modal itself is `view`.
//!
//! Days are local, never UTC: a tick at 11pm is that day's. "Today" is
//! read off the clock at every draw ([`today`]), so the list rolls over
//! by itself after midnight with no day stored anywhere.

pub mod history;
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

/// The todo an agent launch was sent at: which list, which item — and
/// the others selected with it — and the URL of the Linear issue it is
/// linked to, the session's context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TodoRef {
    pub repo_path: std::path::PathBuf,
    pub item: u64,
    pub also: Vec<u64>,
    pub issue_url: Option<String>,
}

/// The DAEMON made the session a launch from the TODOS MODAL asked for:
/// each item it was sent at remembers it — its chip, and `Enter`'s way
/// back to it.
pub(crate) fn agent_started(app: &mut crate::app::App, todo: TodoRef, agent: &orion_core::AgentId) {
    let Some(file) = app.todos.get_mut(&todo.repo_path) else {
        return;
    };
    let mut changed = false;
    for id in std::iter::once(todo.item).chain(todo.also) {
        if let Some(item) = file.item_mut(id) {
            item.agent = Some(agent.0.clone());
            changed = true;
        }
    }
    if changed {
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

    /// Ticked on `today`.
    pub fn done_today(&self, today: NaiveDate) -> bool {
        self.done_on() == Some(today)
    }

    /// In the list: open, or ticked while the modal has been up (`held`)
    /// — struck through at the bottom of its group until the modal goes.
    pub fn shown(&self, held: &[u64]) -> bool {
        self.done.is_none() || held.contains(&self.id)
    }

    /// Whole days an open item has carried over: 0 for one written today.
    pub fn age(&self, today: NaiveDate) -> i64 {
        (today - self.created).num_days().max(0)
    }
}

/// A row of a group's contents: one of its items, or a group under it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Child {
    Item(u64),
    Group(u64),
}

/// Where in a group's open contents a moved item lands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum At {
    Start,
    End,
    Before(Child),
    After(Child),
}

/// What a group `⌥→` makes is called until it is named.
pub const NEW_GROUP: &str = "New group";

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
    /// A file from before [`store::VERSION`] 2, its items put in the order
    /// they were drawn in then: by priority, urgent first, each priority
    /// in the order they stood.
    pub fn upgrade(&mut self) {
        if self.version >= 2 {
            return;
        }
        // Stable: equal priorities keep the order they stand in.
        self.items
            .sort_by_key(|i| crate::linear::priority_rank(i.priority));
        self.version = store::VERSION;
    }

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

    /// The group `group` is nested in, if it is.
    pub fn parent_of(&self, group: u64) -> Option<u64> {
        self.group(group)?.parent
    }

    /// Every item in `group` and the groups under it.
    fn subtree_items(&self, group: u64) -> impl Iterator<Item = &Item> {
        let groups = self.subtree(group);
        self.items.iter().filter(move |i| groups.contains(&i.group))
    }

    /// Everything in it ticked: a group with items, every one of them done
    /// — its nested groups' too. It goes to the bottom of where it stands.
    pub fn complete(&self, group: u64) -> bool {
        let mut items = self.subtree_items(group).peekable();
        items.peek().is_some() && items.all(|i| i.done.is_some())
    }

    /// When a complete group was finished: its last tick.
    fn completed_at(&self, group: u64) -> Option<DateTime<Local>> {
        self.subtree_items(group).filter_map(|i| i.done).max()
    }

    /// In the list: open, or complete with a tick in it from while the
    /// modal has been up (`held`).
    pub fn group_shown(&self, group: u64, held: &[u64]) -> bool {
        !self.complete(group) || self.subtree_items(group).any(|i| held.contains(&i.id))
    }

    /// `group`'s open contents in the order drawn: its open items as they
    /// stand, and each nested group not complete right before the item it
    /// names (`Group::before`) — or the next open one after it — else
    /// after them all.
    pub fn open_children(&self, group: u64) -> Vec<Child> {
        let subs: Vec<&Group> = self
            .subgroups(Some(group))
            .filter(|g| !self.complete(g.id))
            .collect();
        let mut placed = vec![false; subs.len()];
        let mut out = Vec::new();
        let mut waiting = Vec::new();
        for item in self.items.iter().filter(|i| i.group == group) {
            for (k, sub) in subs.iter().enumerate() {
                if !placed[k] && sub.before == Some(item.id) {
                    placed[k] = true;
                    waiting.push(Child::Group(sub.id));
                }
            }
            if item.done.is_none() {
                out.append(&mut waiting);
                out.push(Child::Item(item.id));
            }
        }
        out.append(&mut waiting);
        let rest = subs.iter().zip(&placed).filter(|(_, placed)| !**placed);
        out.extend(rest.map(|(g, _)| Child::Group(g.id)));
        out
    }

    /// What is done under `parent` (`None`: the top) and still shown: the
    /// items ticked while the modal has been up, and the complete groups
    /// with such a tick in them, in the order they were finished.
    fn done_children(&self, parent: Option<u64>, held: &[u64]) -> Vec<Child> {
        let mut done: Vec<(DateTime<Local>, Child)> = Vec::new();
        if let Some(group) = parent {
            let ticked = self
                .items
                .iter()
                .filter(|i| i.group == group && held.contains(&i.id));
            done.extend(ticked.filter_map(|i| Some((i.done?, Child::Item(i.id)))));
        }
        for sub in self.subgroups(parent) {
            if self.complete(sub.id) && self.group_shown(sub.id, held) {
                if let Some(at) = self.completed_at(sub.id) {
                    done.push((at, Child::Group(sub.id)));
                }
            }
        }
        done.sort_by_key(|(at, _)| *at);
        done.into_iter().map(|(_, child)| child).collect()
    }

    /// `group`'s contents as drawn: what is open, then what is done.
    pub fn children(&self, group: u64, held: &[u64]) -> Vec<Child> {
        let mut out = self.open_children(group);
        out.extend(self.done_children(Some(group), held));
        out
    }

    /// The top-level groups as drawn: the open ones in the order they
    /// stand, then the complete ones still shown.
    pub fn top_groups(&self, held: &[u64]) -> Vec<u64> {
        let mut out = self.open_top_groups();
        let done = self.done_children(None, held).into_iter();
        out.extend(done.filter_map(|c| match c {
            Child::Group(g) => Some(g),
            Child::Item(_) => None,
        }));
        out
    }

    /// The top-level groups not complete, in the order they stand.
    fn open_top_groups(&self) -> Vec<u64> {
        self.subgroups(None)
            .filter(|g| !self.complete(g.id))
            .map(|g| g.id)
            .collect()
    }

    /// `group`'s own items as drawn, its nested groups left out.
    pub fn shown_items(&self, group: u64, held: &[u64]) -> Vec<&Item> {
        self.children(group, held)
            .into_iter()
            .filter_map(|c| match c {
                Child::Item(id) => self.item(id),
                Child::Group(_) => None,
            })
            .collect()
    }

    /// A group's open contents put in the order `seq` has them: its items
    /// into the places in [`TodoFile::items`] they held between them, its
    /// groups likewise in [`TodoFile::groups`], each standing before the
    /// item after it in `seq`.
    fn arrange(&mut self, seq: &[Child]) {
        let items: Vec<u64> = seq
            .iter()
            .filter_map(|c| match c {
                Child::Item(id) => Some(*id),
                Child::Group(_) => None,
            })
            .collect();
        let mut slots: Vec<usize> = items.iter().filter_map(|id| self.index_of(*id)).collect();
        slots.sort_unstable();
        let moved: Vec<Item> = items
            .iter()
            .filter_map(|id| self.item(*id).cloned())
            .collect();
        if moved.len() == slots.len() {
            for (slot, item) in slots.into_iter().zip(moved) {
                self.items[slot] = item;
            }
        }
        let groups: Vec<u64> = seq
            .iter()
            .filter_map(|c| match c {
                Child::Group(id) => Some(*id),
                Child::Item(_) => None,
            })
            .collect();
        let mut slots: Vec<usize> = groups
            .iter()
            .filter_map(|id| self.groups.iter().position(|g| g.id == *id))
            .collect();
        slots.sort_unstable();
        let moved: Vec<Group> = groups
            .iter()
            .filter_map(|id| self.group(*id).cloned())
            .collect();
        if moved.len() == slots.len() {
            for (slot, group) in slots.into_iter().zip(moved) {
                self.groups[slot] = group;
            }
        }
        for (k, child) in seq.iter().enumerate() {
            let Child::Group(id) = child else {
                continue;
            };
            let before = seq[k + 1..].iter().find_map(|c| match c {
                Child::Item(id) => Some(*id),
                Child::Group(_) => None,
            });
            if let Some(group) = self.group_mut(*id) {
                group.before = before;
            }
        }
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

    /// How many items were ticked on each day of `today`'s week, Monday
    /// first: the tab row's sparkline. Days still to come are 0.
    pub fn week_ticks(&self, today: NaiveDate) -> [usize; 7] {
        let monday = monday(today);
        let mut days = [0; 7];
        for day in self.items.iter().filter_map(Item::done_on) {
            let Ok(i) = usize::try_from((day - monday).num_days()) else {
                continue;
            };
            if let Some(n) = days.get_mut(i) {
                *n += 1;
            }
        }
        days
    }

    /// Every open item, the count on the project's tab.
    pub fn open_count(&self) -> usize {
        self.items.iter().filter(|i| i.done.is_none()).count()
    }

    /// The names from the top down to `group`: `["MCP fixes", "storyline
    /// prompt"]`.
    pub fn path(&self, group: u64) -> Vec<&str> {
        let mut names: Vec<&str> = self
            .ancestors(group)
            .into_iter()
            .filter_map(|g| self.group(g).map(|g| g.name.as_str()))
            .collect();
        names.reverse();
        names
    }

    /// `group` and every group it is in, innermost first.
    pub fn ancestors(&self, group: u64) -> Vec<u64> {
        let mut out = Vec::new();
        let mut at = self.group(group);
        while let Some(g) = at {
            if out.len() > MAX_DEPTH {
                break;
            }
            out.push(g.id);
            at = g.parent.and_then(|p| self.group(p));
        }
        out
    }

    /// `group` unfolded, and every group it is in: what lands in it is
    /// in sight.
    pub fn reveal(&mut self, group: u64) {
        for id in self.ancestors(group) {
            if let Some(g) = self.group_mut(id) {
                g.collapsed = false;
            }
        }
    }

    /// `group` while it is still there, else the [`TodoFile::inbox`].
    pub fn group_or_inbox(&mut self, group: Option<u64>) -> u64 {
        match group.filter(|g| self.group(*g).is_some()) {
            Some(group) => group,
            None => self.inbox(),
        }
    }

    /// Where `group`'s first item stands in [`TodoFile::items`] — the end
    /// when it has none.
    fn group_start(&self, group: u64) -> usize {
        self.items
            .iter()
            .position(|i| i.group == group)
            .unwrap_or(self.items.len())
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
        self.subtree_items(group).count()
    }

    /// Where `item` stands in [`TodoFile::items`].
    fn index_of(&self, item: u64) -> Option<usize> {
        self.items.iter().position(|i| i.id == item)
    }

    /// A new open item right after `after`, in its group.
    pub fn insert_after(&mut self, after: u64, text: &str, today: NaiveDate) -> Option<u64> {
        let group = self.item(after)?.group;
        let id = self.add_item(group, text, today);
        let item = self.items.pop()?;
        let at = self.index_of(after).map_or(self.items.len(), |i| i + 1);
        self.items.insert(at, item);
        Some(id)
    }

    /// `items` put into `group`: right after `after`, else before the
    /// group's first item, in their order — each as it was, its priority
    /// and all.
    pub fn place(&mut self, items: Vec<Item>, group: u64, after: Option<u64>) {
        let at = match after.and_then(|a| self.index_of(a)) {
            Some(i) => i + 1,
            None => self.group_start(group),
        };
        for (n, mut item) in items.into_iter().enumerate() {
            item.group = group;
            self.items.insert(at + n, item);
        }
    }

    /// `⌥↑`/`⌥↓`: `ids` — in the order drawn — each a place up or down
    /// the list as drawn: past the item beside it; into a nested group it
    /// meets, at the near end, and off a nested group's far end out into
    /// the group around it, right past it; off a top-level group's end,
    /// onto the start of the next one (or the end of the one before).
    /// Nothing moves when the one leading the way has nowhere to go. The
    /// groups they went into.
    pub fn move_items(&mut self, ids: &[u64], up: bool) -> Option<Vec<u64>> {
        let order: Vec<u64> = if up {
            ids.to_vec()
        } else {
            ids.iter().rev().copied().collect()
        };
        let lead = *order.first()?;
        // The lead's step, worked out once: the guard and its own move.
        let mut first = Some(self.step_target(lead, up)?);
        let mut entered = Vec::new();
        for id in order {
            let Some((group, at)) = first.take().or_else(|| self.step_target(id, up)) else {
                continue;
            };
            self.put(id, group, at);
            entered.push(group);
        }
        Some(entered)
    }

    /// Where one step up or down takes open `item`, if anywhere.
    fn step_target(&self, item: u64, up: bool) -> Option<(u64, At)> {
        let group = self.item(item).filter(|i| i.done.is_none())?.group;
        let kids = self.open_children(group);
        let at = kids.iter().position(|c| *c == Child::Item(item))?;
        let beside = if up { at.checked_sub(1) } else { Some(at + 1) };
        match beside.and_then(|i| kids.get(i)).copied() {
            Some(other @ Child::Item(_)) => Some((
                group,
                if up {
                    At::Before(other)
                } else {
                    At::After(other)
                },
            )),
            Some(Child::Group(sub)) => Some((sub, if up { At::End } else { At::Start })),
            None => match self.parent_of(group) {
                Some(parent) => {
                    let me = Child::Group(group);
                    Some((parent, if up { At::Before(me) } else { At::After(me) }))
                }
                None => {
                    let tops = self.open_top_groups();
                    let here = tops.iter().position(|g| *g == group)?;
                    let next = if up {
                        here.checked_sub(1)
                    } else {
                        Some(here + 1)
                    };
                    let next = *next.and_then(|i| tops.get(i))?;
                    Some((next, if up { At::End } else { At::Start }))
                }
            },
        }
    }

    /// Open `item` into `group`'s open contents `at` — the group it left
    /// keeping the rest of its own in order.
    fn put(&mut self, item: u64, group: u64, at: At) {
        let Some(from) = self.item(item).map(|i| i.group) else {
            return;
        };
        // Both worked out before it goes: the group it leaves stands in
        // the one around it even when this was the last open thing in it.
        let mut seq: Vec<Child> = self
            .open_children(group)
            .into_iter()
            .filter(|c| *c != Child::Item(item))
            .collect();
        let left: Vec<Child> = self
            .open_children(from)
            .into_iter()
            .filter(|c| *c != Child::Item(item))
            .collect();
        let index = match at {
            At::Start => 0,
            At::End => seq.len(),
            At::Before(c) => seq.iter().position(|x| *x == c).unwrap_or(seq.len()),
            At::After(c) => seq
                .iter()
                .position(|x| *x == c)
                .map_or(seq.len(), |i| i + 1),
        };
        seq.insert(index, Child::Item(item));
        if from != group {
            if let Some(it) = self.item_mut(item) {
                it.group = group;
            }
            self.arrange(&left);
        }
        self.arrange(&seq);
    }

    /// The group every one of `ids` is in, when they are all open and in
    /// the same one.
    pub fn shared_group(&self, ids: &[u64]) -> Option<u64> {
        let mut groups = ids
            .iter()
            .map(|id| self.item(*id).filter(|i| i.done.is_none()).map(|i| i.group));
        let first = groups.next()??;
        groups.all(|g| g == Some(first)).then_some(first)
    }

    /// `⌥←` on a nested group: it goes, what was in it standing in its
    /// place in the group around it. False when it is not nested.
    pub fn ungroup(&mut self, group: u64) -> bool {
        let Some(parent) = self.parent_of(group) else {
            return false;
        };
        let mut seq = self.open_children(parent);
        let inner = self.open_children(group);
        match seq.iter().position(|c| *c == Child::Group(group)) {
            Some(at) => {
                seq.splice(at..=at, inner);
            }
            None => seq.extend(inner),
        }
        for item in self.items.iter_mut().filter(|i| i.group == group) {
            item.group = parent;
        }
        for sub in self.groups.iter_mut().filter(|g| g.parent == Some(group)) {
            sub.parent = Some(parent);
        }
        self.groups.retain(|g| g.id != group);
        self.arrange(&seq);
        true
    }

    /// `⌥←` on open items in a nested group: out of it into the group
    /// around it, in their order, right after it — the group gone when
    /// that leaves nothing open in it. False when they are not all in one
    /// nested group.
    pub fn outdent(&mut self, ids: &[u64]) -> bool {
        let Some(group) = self.shared_group(ids) else {
            return false;
        };
        let Some(parent) = self.parent_of(group) else {
            return false;
        };
        let mine = |c: &Child| matches!(c, Child::Item(id) if ids.contains(id));
        let kids = self.open_children(group);
        let (moving, left): (Vec<Child>, Vec<Child>) = kids.into_iter().partition(mine);
        if left.is_empty() {
            return self.ungroup(group);
        }
        let mut seq = self.open_children(parent);
        let at = seq
            .iter()
            .position(|c| *c == Child::Group(group))
            .map_or(seq.len(), |i| i + 1);
        seq.splice(at..at, moving);
        for id in ids {
            if let Some(item) = self.item_mut(*id) {
                item.group = parent;
            }
        }
        self.arrange(&left);
        self.arrange(&seq);
        true
    }

    /// `⌥→`: `ids`, open items in one top-level group, into a new group in
    /// it named [`NEW_GROUP`], standing where the first of them stood.
    /// None when they are not — groups nest one deep.
    pub fn group_items(&mut self, ids: &[u64]) -> Option<u64> {
        let group = self.shared_group(ids)?;
        if self.parent_of(group).is_some() {
            return None;
        }
        let mine = |c: &Child| matches!(c, Child::Item(id) if ids.contains(id));
        let kids = self.open_children(group);
        let at = kids.iter().position(mine)?;
        let (inner, mut seq): (Vec<Child>, Vec<Child>) = kids.into_iter().partition(mine);
        let new = self.add_group(Some(group), NEW_GROUP);
        seq.insert(at, Child::Group(new));
        for id in ids {
            if let Some(item) = self.item_mut(*id) {
                item.group = new;
            }
        }
        self.arrange(&seq);
        self.arrange(&inner);
        Some(new)
    }

    /// `item` at Linear's `level` — and, open and now out of order with
    /// the items either side of it, moved to the end of the run of items
    /// at its level, so a list kept in priority order stays so.
    pub fn set_priority(&mut self, item: u64, level: u8) {
        let Some(me) = self.item_mut(item) else {
            return;
        };
        me.priority = level;
        if me.done.is_some() {
            return;
        }
        let group = me.group;
        let rank = |file: &Self, c: &Child| match c {
            Child::Item(id) => file
                .item(*id)
                .map(|i| crate::linear::priority_rank(i.priority)),
            Child::Group(_) => None,
        };
        let mut seq = self.open_children(group);
        let Some(at) = seq.iter().position(|c| *c == Child::Item(item)) else {
            return;
        };
        let mine = crate::linear::priority_rank(level);
        let over = seq[..at].iter().rev().find_map(|c| rank(self, c));
        let under = seq[at + 1..].iter().find_map(|c| rank(self, c));
        if over.is_none_or(|r| r <= mine) && under.is_none_or(|r| mine <= r) {
            return;
        }
        seq.remove(at);
        let to = seq
            .iter()
            .position(|c| rank(self, c).is_some_and(|r| r > mine))
            .or_else(|| {
                seq.iter()
                    .rposition(|c| matches!(c, Child::Item(_)))
                    .map(|i| i + 1)
            })
            .unwrap_or(0);
        seq.insert(to, Child::Item(item));
        self.arrange(&seq);
    }

    /// A new group named `name` under `parent`, after its siblings.
    pub fn add_group(&mut self, parent: Option<u64>, name: &str) -> u64 {
        let id = self.take_id();
        self.groups.push(Group {
            id,
            parent,
            name: name.to_string(),
            collapsed: false,
            before: None,
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

    /// `item` gone — a nested group that stood before it now before the
    /// next item in its group.
    pub fn delete_item(&mut self, item: u64) {
        let Some(at) = self.index_of(item) else {
            return;
        };
        let group = self.items[at].group;
        let next = self.items[at + 1..]
            .iter()
            .find(|i| i.group == group)
            .map(|i| i.id);
        for g in self.groups.iter_mut().filter(|g| g.before == Some(item)) {
            g.before = next;
        }
        self.items.remove(at);
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
        // Groups made here with no item after them yet: the next one
        // added here is the one each stands before, as the list had it.
        let mut waiting: Vec<u64> = Vec::new();
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
                    let id = self.add_item(group, text, today);
                    for sub in waiting.drain(..) {
                        if let Some(sub) = self.group_mut(sub) {
                            sub.before = Some(id);
                        }
                    }
                    added += 1;
                }
                import::Node::Group { name, children } if depth < MAX_DEPTH => {
                    let existing = self
                        .subgroups(parent)
                        .find(|g| g.name == *name)
                        .map(|g| g.id);
                    let group = existing.unwrap_or_else(|| {
                        let made = self.add_group(parent, name);
                        waiting.extend(parent.map(|_| made));
                        made
                    });
                    added += self.import_under(Some(group), children, today, depth + 1);
                }
                import::Node::Group { .. } => {}
            }
        }
        added
    }
}

/// The Monday `today`'s week starts on.
pub fn monday(today: NaiveDate) -> NaiveDate {
    use chrono::Datelike;
    today - chrono::Days::new(u64::from(today.weekday().num_days_from_monday()))
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

    /// A group's contents as drawn, by name: items as their text, nested
    /// groups as `[name]`.
    fn drawn(file: &TodoFile, group: u64, held: &[u64]) -> Vec<String> {
        file.children(group, held)
            .into_iter()
            .map(|c| match c {
                Child::Item(id) => file.item(id).unwrap().text.clone(),
                Child::Group(id) => format!("[{}]", file.group(id).unwrap().name),
            })
            .collect()
    }

    /// A file from before the order was kept is put in the order it was
    /// drawn in — by priority, each priority as it stood — once; from
    /// then on the order is the list's own.
    #[test]
    fn an_old_file_is_put_in_priority_order_once() {
        let mut file = TodoFile::new(Path::new("/r"));
        file.version = 1;
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
        file.upgrade();
        assert_eq!(file.version, store::VERSION);
        assert_eq!(
            drawn(&file, g, &[]),
            ["urgent", "high", "high2", "low", "none"]
        );
        let none = file.items.last().unwrap().id;
        file.move_items(&[none], true).unwrap();
        file.upgrade();
        assert_eq!(
            drawn(&file, g, &[]),
            ["urgent", "high", "high2", "none", "low"]
        );
    }

    /// A tick goes to the bottom of its group at once, in tick order,
    /// while the modal that ticked it is up (`held`); after that it is
    /// out of the list.
    #[test]
    fn a_tick_sinks_while_held_then_goes() {
        let mut file = TodoFile::new(Path::new("/r"));
        let g = file.add_group(None, "G");
        let a = file.add_item(g, "a", day(6));
        let b = file.add_item(g, "b", day(6));
        file.add_item(g, "c", day(6));
        file.toggle(b, at(6, 9));
        file.toggle(a, at(6, 10));
        assert_eq!(drawn(&file, g, &[a, b]), ["c", "b", "a"]);
        assert_eq!(drawn(&file, g, &[]), ["c"]);
        // Unticked, back where it stood.
        file.toggle(a, at(6, 11));
        assert_eq!(drawn(&file, g, &[a, b]), ["a", "c", "b"]);
    }

    /// A group with every item in it ticked is complete: at the bottom of
    /// the group around it — or of the list — while its ticks are held,
    /// then out of sight. An empty group is not complete.
    #[test]
    fn a_complete_group_sinks_then_goes() {
        let mut file = TodoFile::new(Path::new("/r"));
        let top = file.add_group(None, "Top");
        let a = file.add_item(top, "a", day(6));
        let sub = file.add_group(Some(top), "Sub");
        let s1 = file.add_item(sub, "s1", day(6));
        file.add_item(top, "b", day(6));
        let next = file.add_group(None, "Next");
        let empty = file.add_group(None, "Empty");
        assert!(!file.complete(empty));
        assert_eq!(drawn(&file, top, &[]), ["a", "b", "[Sub]"]);
        file.toggle(s1, at(6, 9));
        assert!(file.complete(sub));
        assert_eq!(drawn(&file, top, &[s1]), ["a", "b", "[Sub]"]);
        file.toggle(a, at(6, 8));
        assert_eq!(drawn(&file, top, &[s1, a]), ["b", "a", "[Sub]"]);
        assert_eq!(drawn(&file, top, &[]), ["b"]);
        let b = file.items.iter().find(|i| i.text == "b").unwrap().id;
        file.toggle(b, at(6, 10));
        assert_eq!(file.top_groups(&[b]), [next, empty, top]);
        assert_eq!(file.top_groups(&[]), [next, empty]);
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
        assert_eq!(file.item(a).unwrap().age(day(6)), 5);
    }

    /// The week runs Monday to Sunday: a tick the Sunday before is last
    /// week's, one on Monday the week's first day.
    #[test]
    fn the_week_counts_ticks_from_monday() {
        let mut file = TodoFile::new(Path::new("/r"));
        let g = file.add_group(None, "G");
        // 2026-10-04 is a Sunday, 10-05 a Monday, 10-07 a Wednesday.
        for (d, h) in [(4, 12), (5, 9), (5, 18), (7, 8)] {
            let id = file.add_item(g, "x", day(1));
            file.toggle(id, at(d, h));
        }
        file.add_item(g, "open", day(1));
        assert_eq!(file.week_ticks(day(7)), [2, 0, 1, 0, 0, 0, 0]);
        assert_eq!(file.week_ticks(day(4)), [0, 0, 0, 0, 0, 0, 1]);
        assert_eq!(monday(day(7)), day(5));
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
        assert_eq!(file.shown_items(emails, &[]).len(), 6);
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
            before: None,
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

    /// A nested group pasted between items stands between them.
    #[test]
    fn a_pasted_nested_group_keeps_its_place() {
        let mut file = TodoFile::new(Path::new("/r"));
        let list = "- UI\n    - a\n    - Later\n        - x\n    - b\n";
        file.import(&import::parse(list), day(6));
        let ui = file.subgroups(None).next().unwrap().id;
        assert_eq!(drawn(&file, ui, &[]), ["a", "[Later]", "b"]);
    }

    /// A new item after another is drawn right under it, with no
    /// priority of its own.
    #[test]
    fn an_item_inserted_after_another_is_drawn_under_it() {
        let mut file = TodoFile::new(Path::new("/r"));
        let g = file.add_group(None, "G");
        let a = file.add_item(g, "a", day(6));
        file.add_item(g, "b", day(6));
        file.item_mut(a).unwrap().priority = 2;
        let c = file.insert_after(a, "c", day(6)).unwrap();
        assert_eq!(file.item(c).unwrap().priority, 0);
        assert_eq!(drawn(&file, g, &[]), ["a", "c", "b"]);
        file.insert_after(c, "d", day(6)).unwrap();
        assert_eq!(drawn(&file, g, &[]), ["a", "c", "d", "b"]);
    }

    /// `⌥↓` walks an item down the list as drawn, whatever its priority:
    /// past the items, into a nested group at its top, through it, out
    /// of its bottom right under it, on past the rest of the group, then
    /// onto the start of the next. `⌥↑` walks the same way back.
    #[test]
    fn an_item_moves_through_a_nested_group_and_out() {
        let mut file = TodoFile::new(Path::new("/r"));
        let top = file.add_group(None, "A");
        let a1 = file.add_item(top, "a1", day(6));
        let x = file.add_item(top, "x", day(6));
        file.item_mut(x).unwrap().priority = 1;
        let a2 = file.add_item(top, "a2", day(6));
        let a3 = file.add_item(top, "a3", day(6));
        let sub = file.add_group(Some(top), "Sub");
        file.add_item(sub, "s1", day(6));
        file.add_item(sub, "s2", day(6));
        // Sub stands between a2 and a3.
        file.group_mut(sub).unwrap().before = Some(a3);
        let next = file.add_group(None, "B");
        file.add_item(next, "b1", day(6));
        assert_eq!(drawn(&file, top, &[]), ["a1", "x", "a2", "[Sub]", "a3"]);

        let down = |file: &mut TodoFile| file.move_items(&[x], false).unwrap();
        down(&mut file);
        assert_eq!(drawn(&file, top, &[]), ["a1", "a2", "x", "[Sub]", "a3"]);
        assert_eq!(down(&mut file), [sub]);
        assert_eq!(drawn(&file, sub, &[]), ["x", "s1", "s2"]);
        down(&mut file);
        down(&mut file);
        assert_eq!(drawn(&file, sub, &[]), ["s1", "s2", "x"]);
        assert_eq!(down(&mut file), [top]);
        assert_eq!(drawn(&file, top, &[]), ["a1", "a2", "[Sub]", "x", "a3"]);
        assert_eq!(drawn(&file, sub, &[]), ["s1", "s2"]);
        down(&mut file);
        assert_eq!(drawn(&file, top, &[]), ["a1", "a2", "[Sub]", "a3", "x"]);
        assert_eq!(down(&mut file), [next]);
        assert_eq!(drawn(&file, next, &[]), ["x", "b1"]);
        assert_eq!(file.item(x).unwrap().priority, 1, "its priority kept");

        // And back up: onto A's end, then into Sub's bottom.
        let up = |file: &mut TodoFile| file.move_items(&[x], true).unwrap();
        assert_eq!(up(&mut file), [top]);
        up(&mut file);
        assert_eq!(drawn(&file, top, &[]), ["a1", "a2", "[Sub]", "x", "a3"]);
        assert_eq!(up(&mut file), [sub]);
        assert_eq!(drawn(&file, sub, &[]), ["s1", "s2", "x"]);
        up(&mut file);
        up(&mut file);
        up(&mut file);
        assert_eq!(drawn(&file, top, &[]), ["a1", "a2", "x", "[Sub]", "a3"]);
        // The first group's top has nowhere to go: nothing moves.
        assert!(file.move_items(&[a1, a2], true).is_none());
        let _ = a2;
    }

    /// A selection moves as one: down into a nested group and out again,
    /// in its order.
    #[test]
    fn a_selection_moves_together() {
        let mut file = TodoFile::new(Path::new("/r"));
        let top = file.add_group(None, "A");
        let a = file.add_item(top, "a", day(6));
        let b = file.add_item(top, "b", day(6));
        let sub = file.add_group(Some(top), "Sub");
        file.add_item(sub, "s", day(6));
        file.move_items(&[a, b], false).unwrap();
        assert_eq!(drawn(&file, sub, &[]), ["a", "b", "s"]);
        file.move_items(&[a, b], false).unwrap();
        file.move_items(&[a, b], false).unwrap();
        assert_eq!(drawn(&file, top, &[]), ["[Sub]", "a", "b"]);
        assert_eq!(drawn(&file, sub, &[]), ["s"]);
    }

    /// `⌥→` groups items of a top-level group where the first stood, and
    /// not items already in a nested group; `⌥←` takes some out, right
    /// under the group, and all of them — or the group itself — flattens
    /// it into its place.
    #[test]
    fn items_group_and_ungroup() {
        let mut file = TodoFile::new(Path::new("/r"));
        let top = file.add_group(None, "A");
        let a = file.add_item(top, "a", day(6));
        let b = file.add_item(top, "b", day(6));
        let c = file.add_item(top, "c", day(6));
        let d = file.add_item(top, "d", day(6));
        let sub = file.group_items(&[b, c]).unwrap();
        assert_eq!(file.group(sub).unwrap().name, NEW_GROUP);
        assert_eq!(drawn(&file, top, &[]), ["a", "[New group]", "d"]);
        assert_eq!(drawn(&file, sub, &[]), ["b", "c"]);
        assert!(file.group_items(&[b]).is_none(), "one deep");
        assert!(file.group_items(&[a, b]).is_none(), "one group at a time");

        assert!(file.outdent(&[b]));
        assert_eq!(drawn(&file, top, &[]), ["a", "[New group]", "b", "d"]);
        assert!(!file.outdent(&[a]), "already at the top");
        assert!(file.outdent(&[c]));
        assert!(file.group(sub).is_none(), "nothing left open in it");
        assert_eq!(drawn(&file, top, &[]), ["a", "c", "b", "d"]);

        let sub = file.group_items(&[c, b]).unwrap();
        assert!(file.ungroup(sub));
        assert_eq!(drawn(&file, top, &[]), ["a", "c", "b", "d"]);
        assert!(!file.ungroup(top));
        let _ = d;
    }

    /// A priority set moves the item to the end of its level's run when
    /// it stood out of order — and leaves it be when it did not.
    #[test]
    fn a_priority_keeps_the_list_in_order() {
        let mut file = TodoFile::new(Path::new("/r"));
        let g = file.add_group(None, "G");
        let ids: Vec<u64> = ["a", "b", "c"]
            .iter()
            .map(|t| file.add_item(g, t, day(6)))
            .collect();
        file.set_priority(ids[2], 2);
        assert_eq!(drawn(&file, g, &[]), ["c", "a", "b"]);
        file.set_priority(ids[1], 1);
        assert_eq!(drawn(&file, g, &[]), ["b", "c", "a"]);
        file.set_priority(ids[0], 2);
        assert_eq!(drawn(&file, g, &[]), ["b", "c", "a"], "in order already");
        file.set_priority(ids[1], 0);
        assert_eq!(drawn(&file, g, &[]), ["c", "a", "b"]);
    }

    /// Pasted items go after the one named, in their order — or to the
    /// top of the group — a nested group standing where it stood.
    #[test]
    fn placed_items_land_after_the_one_named() {
        let mut file = TodoFile::new(Path::new("/r"));
        let g = file.add_group(None, "G");
        let h = file.add_group(None, "H");
        let a = file.add_item(g, "a", day(6));
        let b = file.add_item(g, "b", day(6));
        let sub = file.add_group(Some(g), "Sub");
        file.group_mut(sub).unwrap().before = Some(b);
        let x = file.add_item(h, "x", day(6));
        let y = file.add_item(h, "y", day(6));
        let moved: Vec<Item> = [x, y]
            .iter()
            .map(|id| file.item(*id).unwrap().clone())
            .collect();
        file.delete_item(x);
        file.delete_item(y);
        file.place(moved.clone(), g, Some(a));
        assert_eq!(drawn(&file, g, &[]), ["a", "x", "y", "[Sub]", "b"]);
        file.delete_item(x);
        file.delete_item(y);
        file.place(moved, h, None);
        assert_eq!(texts(&file.shown_items(h, &[])), ["x", "y"]);
        // Its item gone, a nested group stands before the next one.
        file.add_item(g, "c", day(6));
        file.delete_item(b);
        assert_eq!(drawn(&file, g, &[]), ["a", "[Sub]", "c"]);
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
