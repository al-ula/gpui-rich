//! Optional CRDT synchronization bridge built over [`CommandMiddleware::after_transaction`].
//!
//! Uses a Last-Writer-Wins (LWW) Element-Set with tombstones and deterministic fractional
//! [`OrderKey`]s so concurrent edits across replicas converge to identical [`Document`] state
//! regardless of network delivery order.

use crate::block::{BlockId, BlockNode, Document};
use crate::command::CommandMiddleware;
use crate::selection::DocumentSelection;
use crate::style::StyleSheet;
use crate::transaction::{ChangeSet, Step, Transaction};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};

/// Unique identifier for a collaborating replica.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReplicaId(pub u64);

/// Lamport logical clock with deterministic replica tie-breaking.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct SyncClock {
    pub counter: u64,
    pub replica: ReplicaId,
}

impl SyncClock {
    pub const fn new(counter: u64, replica: ReplicaId) -> Self {
        Self { counter, replica }
    }
}

/// Lexicographic fractional ordering key with replica tie-breaker for concurrent block inserts.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct OrderKey {
    pub path: Vec<u32>,
    pub replica: ReplicaId,
}

impl OrderKey {
    /// Generates a key strictly between `left` and `right` (either of which may be `None`).
    pub fn between(left: Option<&Self>, right: Option<&Self>, replica: ReplicaId) -> Self {
        const STEP: u32 = 1024;
        let left_path = left.map(|key| key.path.as_slice()).unwrap_or(&[]);
        let right_path = right.map(|key| key.path.as_slice()).unwrap_or(&[]);
        let max_len = left_path.len().max(right_path.len());
        let mut path = Vec::with_capacity(max_len + 1);

        for index in 0..=max_len {
            let low = left_path.get(index).copied().unwrap_or(0);
            let high = right_path.get(index).copied().unwrap_or(u32::MAX);
            if high > low.saturating_add(1) {
                let mid = if left.is_none() && right.is_none() {
                    STEP
                } else if right.is_none() {
                    low.saturating_add(STEP)
                } else if left.is_none() {
                    high / 2
                } else {
                    low + (high - low) / 2
                };
                if mid > low && mid < high {
                    path.push(mid);
                    return Self { path, replica };
                }
            }
            path.push(low);
        }
        path.push(STEP);
        Self { path, replica }
    }
}

/// Atomic CRDT operation transmitted between replicas.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SyncOp {
    UpsertBlock {
        block_id: BlockId,
        order_key: OrderKey,
        block: Box<BlockNode>,
        clock: SyncClock,
    },
    DeleteBlock {
        block_id: BlockId,
        clock: SyncClock,
    },
    UpdateStyleSheet {
        stylesheet: StyleSheet,
        clock: SyncClock,
    },
}

/// Serializable batch of CRDT operations emitted by a replica.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SyncDelta {
    pub origin: ReplicaId,
    pub clock: u64,
    pub ops: Vec<SyncOp>,
}

impl SyncDelta {
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string(self)
    }

    pub fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str(json)
    }
}

/// Optional transport sink receiving outgoing [`SyncDelta`] packets from [`CrdtSyncBridge`].
pub trait SyncTransport {
    fn send_delta(&mut self, delta: SyncDelta);
}

#[derive(Clone, Debug)]
struct CrdtBlockEntry {
    order_key: OrderKey,
    block: BlockNode,
    clock: SyncClock,
    deleted: bool,
}

/// CRDT sync adapter implementing [`CommandMiddleware`] to observe local `after_transaction`
/// mutations and apply incoming remote [`SyncDelta`] packets convergently.
pub struct CrdtSyncBridge {
    replica: ReplicaId,
    logical_clock: u64,
    entries: HashMap<BlockId, CrdtBlockEntry>,
    stylesheet_clock: Option<SyncClock>,
    last_stylesheet: StyleSheet,
    outbox: Vec<SyncDelta>,
    transport: Option<Box<dyn SyncTransport>>,
    applying_remote: bool,
}

impl CrdtSyncBridge {
    /// Initializes a sync bridge seeded from the initial `document` state.
    pub fn new(replica: ReplicaId, document: &Document) -> Self {
        let mut entries = HashMap::with_capacity(document.blocks.len());
        let mut prev_key: Option<OrderKey> = None;
        for block in &document.blocks {
            let key = OrderKey::between(prev_key.as_ref(), None, ReplicaId(0));
            entries.insert(
                block.id,
                CrdtBlockEntry {
                    order_key: key.clone(),
                    block: block.clone(),
                    clock: SyncClock::new(0, ReplicaId(0)),
                    deleted: false,
                },
            );
            prev_key = Some(key);
        }
        Self {
            replica,
            logical_clock: 0,
            entries,
            stylesheet_clock: None,
            last_stylesheet: document.stylesheet.clone(),
            outbox: Vec::new(),
            transport: None,
            applying_remote: false,
        }
    }

    /// Attaches a [`SyncTransport`] that immediately receives each outgoing [`SyncDelta`].
    pub fn with_transport(mut self, transport: impl SyncTransport + 'static) -> Self {
        self.transport = Some(Box::new(transport));
        self
    }

    /// Returns this bridge's [`ReplicaId`].
    pub const fn replica(&self) -> ReplicaId {
        self.replica
    }

    /// Drains all queued outgoing [`SyncDelta`] packets produced by local transactions.
    pub fn drain_outbox(&mut self) -> Vec<SyncDelta> {
        std::mem::take(&mut self.outbox)
    }

    /// Marks whether a remote transaction is currently being applied so `after_transaction`
    /// does not re-broadcast remote operations back to peers.
    pub fn set_applying_remote(&mut self, applying: bool) {
        self.applying_remote = applying;
    }

    /// Exports a full snapshot [`SyncDelta`] containing all live and tombstoned entries.
    pub fn export_snapshot_delta(&self) -> SyncDelta {
        let mut ops = Vec::with_capacity(self.entries.len());
        for (&block_id, entry) in &self.entries {
            if entry.deleted {
                ops.push(SyncOp::DeleteBlock {
                    block_id,
                    clock: entry.clock,
                });
            } else {
                ops.push(SyncOp::UpsertBlock {
                    block_id,
                    order_key: entry.order_key.clone(),
                    block: Box::new(entry.block.clone()),
                    clock: entry.clock,
                });
            }
        }
        SyncDelta {
            origin: self.replica,
            clock: self.logical_clock,
            ops,
        }
    }

    /// Merges a remote [`SyncDelta`] into CRDT state and builds a [`Transaction`] updating
    /// `document` to the converged state (returns `None` if no state changed).
    pub fn apply_remote_delta(
        &mut self,
        delta: &SyncDelta,
        document: &Document,
    ) -> Option<Transaction> {
        self.logical_clock = self.logical_clock.max(delta.clock);
        let mut changed = false;
        let mut updated_stylesheet: Option<StyleSheet> = None;

        for op in &delta.ops {
            match op {
                SyncOp::UpsertBlock {
                    block_id,
                    order_key,
                    block,
                    clock,
                } => {
                    self.logical_clock = self.logical_clock.max(clock.counter);
                    let accept = self
                        .entries
                        .get(block_id)
                        .is_none_or(|existing| *clock > existing.clock);
                    if accept {
                        self.entries.insert(
                            *block_id,
                            CrdtBlockEntry {
                                order_key: order_key.clone(),
                                block: (**block).clone(),
                                clock: *clock,
                                deleted: false,
                            },
                        );
                        changed = true;
                    }
                }
                SyncOp::DeleteBlock { block_id, clock } => {
                    self.logical_clock = self.logical_clock.max(clock.counter);
                    let accept = self
                        .entries
                        .get(block_id)
                        .is_none_or(|existing| *clock > existing.clock);
                    if accept {
                        if let Some(existing) = self.entries.get_mut(block_id) {
                            existing.clock = *clock;
                            existing.deleted = true;
                        } else {
                            self.entries.insert(
                                *block_id,
                                CrdtBlockEntry {
                                    order_key: OrderKey::between(None, None, clock.replica),
                                    block: BlockNode::new(
                                        *block_id,
                                        crate::block::BlockKind::horizontal_rule(),
                                        crate::style::BlockStyle::default(),
                                    ),
                                    clock: *clock,
                                    deleted: true,
                                },
                            );
                        }
                        changed = true;
                    }
                }
                SyncOp::UpdateStyleSheet { stylesheet, clock } => {
                    self.logical_clock = self.logical_clock.max(clock.counter);
                    if self.stylesheet_clock.is_none_or(|prev| *clock > prev) {
                        self.stylesheet_clock = Some(*clock);
                        self.last_stylesheet = stylesheet.clone();
                        updated_stylesheet = Some(stylesheet.clone());
                        changed = true;
                    }
                }
            }
        }

        if !changed {
            return None;
        }

        // Order live CRDT entries deterministically by `(order_key, block_id)`.
        let mut live: Vec<&CrdtBlockEntry> = self
            .entries
            .values()
            .filter(|entry| !entry.deleted)
            .collect();
        live.sort_by(|left, right| {
            left.order_key
                .cmp(&right.order_key)
                .then_with(|| left.block.id.cmp(&right.block.id))
        });

        let mut steps = Vec::new();
        if let Some(sheet) = updated_stylesheet {
            for (style_id, named_style) in sheet.styles {
                if document.stylesheet.get(style_id) != Some(&named_style) {
                    steps.push(Step::UpdateNamedStyle {
                        style_id,
                        style: Some(named_style),
                    });
                }
            }
        }

        let current_ids: Vec<BlockId> = document.blocks.iter().map(|block| block.id).collect();
        let target_ids: Vec<BlockId> = live.iter().map(|entry| entry.block.id).collect();

        if current_ids == target_ids {
            // Fast path: order unchanged; patch only modified blocks in place.
            for (current, target) in document.blocks.iter().zip(live.iter()) {
                if current.kind != target.block.kind {
                    steps.push(Step::SetBlockKind {
                        block_id: current.id,
                        kind: target.block.kind.clone(),
                    });
                }
                if current.style != target.block.style {
                    steps.push(Step::SetBlockStyle {
                        block_id: current.id,
                        style: target.block.style.clone(),
                    });
                }
            }
        } else {
            // Rebuild top-level block list in exact converged order.
            for remove_idx in (0..document.blocks.len()).rev() {
                steps.push(Step::RemoveBlock { index: remove_idx });
            }
            for (insert_idx, entry) in live.into_iter().enumerate() {
                steps.push(Step::InsertBlock {
                    index: insert_idx,
                    block: entry.block.clone(),
                });
            }
        }

        if steps.is_empty() {
            None
        } else {
            Some(Transaction::new(steps))
        }
    }

    fn next_clock(&mut self) -> SyncClock {
        self.logical_clock = self.logical_clock.saturating_add(1);
        SyncClock::new(self.logical_clock, self.replica)
    }

    fn record_local_document_diff(&mut self, change_set: &ChangeSet, document: &Document) {
        let mut ops = Vec::new();
        let present_ids: HashSet<BlockId> = document.blocks.iter().map(|block| block.id).collect();

        // Detect deleted top-level blocks.
        let deleted_ids: Vec<BlockId> = self
            .entries
            .iter()
            .filter(|(id, entry)| !entry.deleted && !present_ids.contains(id))
            .map(|(id, _)| *id)
            .collect();
        for block_id in deleted_ids {
            let clock = self.next_clock();
            if let Some(entry) = self.entries.get_mut(&block_id) {
                entry.clock = clock;
                entry.deleted = true;
            }
            ops.push(SyncOp::DeleteBlock { block_id, clock });
        }

        // Detect inserted or modified top-level blocks (preserving fractional order keys).
        let mut resolved_keys: Vec<Option<OrderKey>> = document
            .blocks
            .iter()
            .map(|block| {
                self.entries
                    .get(&block.id)
                    .filter(|entry| !entry.deleted)
                    .map(|entry| entry.order_key.clone())
            })
            .collect();

        for index in 0..document.blocks.len() {
            let block = &document.blocks[index];
            let is_new = resolved_keys[index].is_none();
            let is_dirty = is_new
                || change_set.dirty_blocks.contains(&block.id)
                || self.entries.get(&block.id).is_none_or(|entry| {
                    entry.deleted
                        || entry.block.kind != block.kind
                        || entry.block.style != block.style
                });

            if is_new {
                let left = index
                    .checked_sub(1)
                    .and_then(|prev| resolved_keys.get(prev))
                    .and_then(Option::as_ref);
                let right = resolved_keys
                    .iter()
                    .skip(index + 1)
                    .find_map(Option::as_ref);
                let new_key = OrderKey::between(left, right, self.replica);
                resolved_keys[index] = Some(new_key);
            }

            if is_dirty && let Some(order_key) = resolved_keys[index].clone() {
                let clock = self.next_clock();
                self.entries.insert(
                    block.id,
                    CrdtBlockEntry {
                        order_key: order_key.clone(),
                        block: block.clone(),
                        clock,
                        deleted: false,
                    },
                );
                ops.push(SyncOp::UpsertBlock {
                    block_id: block.id,
                    order_key,
                    block: Box::new(block.clone()),
                    clock,
                });
            }
        }

        if document.stylesheet != self.last_stylesheet {
            let clock = self.next_clock();
            self.stylesheet_clock = Some(clock);
            self.last_stylesheet = document.stylesheet.clone();
            ops.push(SyncOp::UpdateStyleSheet {
                stylesheet: document.stylesheet.clone(),
                clock,
            });
        }

        if !ops.is_empty() {
            let delta = SyncDelta {
                origin: self.replica,
                clock: self.logical_clock,
                ops,
            };
            if let Some(transport) = self.transport.as_mut() {
                transport.send_delta(delta.clone());
            }
            self.outbox.push(delta);
        }
    }
}

impl CommandMiddleware for CrdtSyncBridge {
    fn after_transaction(
        &mut self,
        change_set: &ChangeSet,
        document: &Document,
        _selection: Option<&DocumentSelection>,
    ) {
        if self.applying_remote {
            return;
        }
        self.record_local_document_diff(change_set, document);
    }
}
