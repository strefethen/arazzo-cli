use std::collections::BTreeMap;
use std::fs;

use arazzo_runtime::{StepBreakpoint, StepCheckpoint};
use yaml_rust2::parser::{Event as YamlEvent, MarkedEventReceiver, Parser as YamlParser};
use yaml_rust2::scanner::Marker as YamlMarker;

use super::requests::DapBreakpoint;
use super::responses::ResolvedBreakpoint;

const BREAKPOINT_NEAREST_LINE_THRESHOLD: u32 = 10;

#[derive(Debug, Clone)]
pub(super) struct IndexedCheckpoint {
    pub(super) line: u32,
    pub(super) workflow_id: String,
    pub(super) step_id: String,
    pub(super) checkpoint: StepCheckpoint,
}

#[derive(Debug, Clone)]
pub(super) struct SourceIndex {
    pub(super) path: String,
    pub(super) checkpoints: Vec<IndexedCheckpoint>,
    pub(super) line_contexts: BTreeMap<u32, SourceLineContext>,
    pub(super) output_expressions: BTreeMap<(String, String, String), String>,
}

#[derive(Debug, Clone)]
pub(super) struct SourceLineContext {
    pub(super) workflow_id: String,
    pub(super) step_id: String,
    pub(super) area: BreakpointArea,
    pub(super) prefer_forward_snap: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum BreakpointArea {
    Step,
    SuccessCriteria,
    OnSuccess,
    OnFailure,
    Outputs,
}

impl BreakpointArea {
    fn label(self) -> &'static str {
        match self {
            Self::Step => "step",
            Self::SuccessCriteria => "successCriteria",
            Self::OnSuccess => "onSuccess",
            Self::OnFailure => "onFailure",
            Self::Outputs => "outputs",
        }
    }
}

#[derive(Debug, Default)]
pub(super) struct ResolvedSourceBreakpoints {
    pub(super) resolved: Vec<ResolvedBreakpoint>,
    pub(super) runtime: Vec<StepBreakpoint>,
}

pub(super) fn build_source_index(path: &str) -> Result<SourceIndex, String> {
    let text =
        fs::read_to_string(path).map_err(|err| format!("reading source index file: {err}"))?;
    let metadata = extract_source_metadata(&text);
    Ok(SourceIndex {
        path: path.to_string(),
        checkpoints: metadata.checkpoints,
        line_contexts: metadata.line_contexts,
        output_expressions: metadata.output_expressions,
    })
}

pub(super) fn try_build_source_index(path: &str) -> Option<SourceIndex> {
    // Intentional: source index failures should not block launch or breakpoint setup.
    // The adapter returns verified placeholders and resolves at runtime instead.
    build_source_index(path).ok()
}

pub(super) fn lookup_line_for_checkpoint(
    source_index: Option<&SourceIndex>,
    workflow_id: &str,
    step_id: &str,
    checkpoint: &StepCheckpoint,
) -> Option<u32> {
    let index = source_index?;
    let exact = index
        .checkpoints
        .iter()
        .find(|candidate| {
            candidate.workflow_id == workflow_id
                && candidate.step_id == step_id
                && candidate.checkpoint == *checkpoint
        })
        .or_else(|| {
            retry_lifecycle_action_checkpoint(checkpoint).and_then(|action_checkpoint| {
                index.checkpoints.iter().find(|candidate| {
                    candidate.workflow_id == workflow_id
                        && candidate.step_id == step_id
                        && candidate.checkpoint == action_checkpoint
                })
            })
        });
    if let Some(value) = exact {
        return Some(value.line);
    }
    let fallback = index.checkpoints.iter().find(|candidate| {
        candidate.workflow_id == workflow_id
            && candidate.step_id == step_id
            && matches!(candidate.checkpoint, StepCheckpoint::Step)
    });
    fallback.map(|value| value.line)
}

pub(super) fn lookup_output_expression<'a>(
    source_index: Option<&'a SourceIndex>,
    workflow_id: &str,
    step_id: &str,
    output_name: &str,
) -> Option<&'a str> {
    let index = source_index?;
    index
        .output_expressions
        .get(&(
            workflow_id.to_string(),
            step_id.to_string(),
            output_name.to_string(),
        ))
        .map(String::as_str)
}

/// Resolves DAP source-line breakpoints against the YAML source index, producing
/// both the editor-facing [`ResolvedBreakpoint`] list and the runtime
/// [`StepBreakpoint`] list. Falls back to verified placeholders if the source
/// index cannot be built so launch is not blocked.
pub(super) fn resolve_source_breakpoints(
    source_path: &str,
    requested: &[DapBreakpoint],
    launch_workflow: Option<&str>,
    existing_index: Option<&SourceIndex>,
) -> ResolvedSourceBreakpoints {
    let mut index = existing_index
        .cloned()
        .filter(|idx| idx.path == source_path);
    if index.is_none() {
        index = try_build_source_index(source_path);
    }

    let Some(index) = index else {
        let resolved = requested
            .iter()
            .map(|bp| ResolvedBreakpoint {
                line: bp.line,
                verified: true,
                message: Some("source index unavailable; deferred mapping".to_string()),
            })
            .collect::<Vec<_>>();
        return ResolvedSourceBreakpoints {
            resolved,
            runtime: Vec::new(),
        };
    };

    let mut resolved = Vec::<ResolvedBreakpoint>::new();
    let mut runtime_breakpoints = Vec::<StepBreakpoint>::new();
    for bp in requested {
        let line_context = resolve_line_context(bp.line, &index, launch_workflow);
        let Some(checkpoint) = resolve_breakpoint_checkpoint(bp.line, &index, launch_workflow)
        else {
            let message = invalid_breakpoint_message(line_context.as_ref());
            resolved.push(ResolvedBreakpoint {
                line: bp.line,
                verified: false,
                message: Some(message),
            });
            continue;
        };

        let mut runtime_bp =
            StepBreakpoint::new(checkpoint.workflow_id.clone(), checkpoint.step_id.clone());
        runtime_bp.checkpoint = checkpoint.checkpoint.clone();
        if let Some(condition) = bp
            .condition
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            runtime_bp = runtime_bp.with_condition(condition.clone());
        }
        runtime_breakpoints.push(runtime_bp);

        let mut parts = Vec::<String>::new();
        if checkpoint.line != bp.line {
            parts.push(format!(
                "mapped line {} to {} on line {}",
                bp.line,
                checkpoint_display_name(&checkpoint.checkpoint),
                checkpoint.line
            ));
        }
        if let Some(condition) = bp
            .condition
            .as_ref()
            .filter(|value| !value.trim().is_empty())
        {
            parts.push(format!(
                "condition on {}: {}",
                checkpoint_display_name(&checkpoint.checkpoint),
                condition
            ));
        }
        let message = (!parts.is_empty()).then(|| parts.join("; "));
        resolved.push(ResolvedBreakpoint {
            line: checkpoint.line,
            verified: true,
            message,
        });
    }

    ResolvedSourceBreakpoints {
        resolved,
        runtime: runtime_breakpoints,
    }
}

fn resolve_breakpoint_checkpoint(
    line: u32,
    index: &SourceIndex,
    workflow_filter: Option<&str>,
) -> Option<IndexedCheckpoint> {
    let mut candidates = index
        .checkpoints
        .iter()
        .filter(|candidate| {
            workflow_filter.is_none_or(|workflow_id| candidate.workflow_id == workflow_id)
        })
        .cloned()
        .collect::<Vec<_>>();
    candidates.sort_by_key(|candidate| candidate.line);

    if let Some(exact) = candidates.iter().find(|candidate| candidate.line == line) {
        return Some(exact.clone());
    }

    let line_context = resolve_line_context(line, index, workflow_filter);
    if let Some(ctx) = line_context.as_ref() {
        let same_step = candidates
            .iter()
            .filter(|candidate| {
                candidate.workflow_id == ctx.workflow_id && candidate.step_id == ctx.step_id
            })
            .cloned()
            .collect::<Vec<_>>();
        let same_area = same_step
            .iter()
            .filter(|candidate| checkpoint_area(&candidate.checkpoint) == ctx.area)
            .cloned()
            .collect::<Vec<_>>();
        if !same_area.is_empty() {
            candidates = same_area;
        } else if !same_step.is_empty() {
            candidates = same_step;
        }
    }

    let prefer_forward = line_context
        .as_ref()
        .map(|ctx| ctx.prefer_forward_snap)
        .unwrap_or(false);

    let mut best: Option<IndexedCheckpoint> = None;
    let mut best_distance = u32::MAX;
    for candidate in candidates {
        let distance = candidate.line.abs_diff(line);
        if distance < best_distance
            || (distance == best_distance
                && is_better_direction_tiebreak(best.as_ref(), &candidate, line, prefer_forward))
        {
            best = Some(candidate);
            best_distance = distance;
        }
    }
    if best_distance <= BREAKPOINT_NEAREST_LINE_THRESHOLD {
        best
    } else {
        None
    }
}

fn resolve_line_context(
    line: u32,
    index: &SourceIndex,
    workflow_filter: Option<&str>,
) -> Option<SourceLineContext> {
    if let Some(exact) = index
        .line_contexts
        .get(&line)
        .filter(|ctx| workflow_filter.is_none_or(|workflow_id| ctx.workflow_id == workflow_id))
    {
        return Some(exact.clone());
    }

    let mut best: Option<&SourceLineContext> = None;
    let mut best_line = 0u32;
    let mut best_distance = u32::MAX;
    for (&ctx_line, ctx) in &index.line_contexts {
        if workflow_filter.is_some_and(|workflow_id| ctx.workflow_id != workflow_id) {
            continue;
        }
        let distance = ctx_line.abs_diff(line);
        if distance > BREAKPOINT_NEAREST_LINE_THRESHOLD {
            continue;
        }
        if distance < best_distance
            || (distance == best_distance
                && is_better_line_tiebreak(best_line, ctx_line, line, false))
        {
            best = Some(ctx);
            best_line = ctx_line;
            best_distance = distance;
        }
    }
    best.cloned()
}

fn checkpoint_area(checkpoint: &StepCheckpoint) -> BreakpointArea {
    match checkpoint {
        StepCheckpoint::Step => BreakpointArea::Step,
        StepCheckpoint::SuccessCriterion { .. } => BreakpointArea::SuccessCriteria,
        StepCheckpoint::OnSuccessAction { .. }
        | StepCheckpoint::OnSuccessCriterion { .. }
        | StepCheckpoint::OnSuccessRetrySelected { .. }
        | StepCheckpoint::OnSuccessRetryDelay { .. } => BreakpointArea::OnSuccess,
        StepCheckpoint::OnFailureAction { .. }
        | StepCheckpoint::OnFailureCriterion { .. }
        | StepCheckpoint::OnFailureRetrySelected { .. }
        | StepCheckpoint::OnFailureRetryDelay { .. } => BreakpointArea::OnFailure,
        StepCheckpoint::Output { .. } => BreakpointArea::Outputs,
        _ => BreakpointArea::Step,
    }
}

fn is_better_direction_tiebreak(
    current_best: Option<&IndexedCheckpoint>,
    candidate: &IndexedCheckpoint,
    line: u32,
    prefer_forward: bool,
) -> bool {
    let Some(best) = current_best else {
        return true;
    };
    is_better_line_tiebreak(best.line, candidate.line, line, prefer_forward)
}

fn is_better_line_tiebreak(
    current_best_line: u32,
    candidate_line: u32,
    target_line: u32,
    prefer_forward: bool,
) -> bool {
    let current_best_is_forward = current_best_line >= target_line;
    let candidate_is_forward = candidate_line >= target_line;
    if current_best_is_forward != candidate_is_forward {
        return candidate_is_forward == prefer_forward;
    }
    candidate_line < current_best_line
}

fn invalid_breakpoint_message(line_context: Option<&SourceLineContext>) -> String {
    if let Some(ctx) = line_context {
        return format!(
            "no executable checkpoint near this line in {} block; use step, criteria item, action item, or output entry lines",
            ctx.area.label()
        );
    }
    "breakpoint must be on or near step, successCriteria, onSuccess, onFailure, or outputs"
        .to_string()
}

pub(super) fn checkpoint_display_name(checkpoint: &StepCheckpoint) -> String {
    match checkpoint {
        StepCheckpoint::Step => "step".to_string(),
        StepCheckpoint::SuccessCriterion { index } => format!("successCriteria[{index}]"),
        StepCheckpoint::OnSuccessAction { index } => format!("onSuccess[{index}]"),
        StepCheckpoint::OnSuccessCriterion {
            action_index,
            criterion_index,
        } => format!("onSuccess[{action_index}].criteria[{criterion_index}]"),
        StepCheckpoint::OnFailureAction { index } => format!("onFailure[{index}]"),
        StepCheckpoint::OnFailureCriterion {
            action_index,
            criterion_index,
        } => format!("onFailure[{action_index}].criteria[{criterion_index}]"),
        StepCheckpoint::OnSuccessRetrySelected { action_index } => {
            format!("onSuccess[{action_index}].retrySelected")
        }
        StepCheckpoint::OnSuccessRetryDelay { action_index } => {
            format!("onSuccess[{action_index}].retryDelay")
        }
        StepCheckpoint::OnFailureRetrySelected { action_index } => {
            format!("onFailure[{action_index}].retrySelected")
        }
        StepCheckpoint::OnFailureRetryDelay { action_index } => {
            format!("onFailure[{action_index}].retryDelay")
        }
        StepCheckpoint::Output { name } => format!("outputs.{name}"),
        _ => "step".to_string(),
    }
}

pub(super) fn checkpoint_sort_key(checkpoint: &StepCheckpoint) -> String {
    match checkpoint {
        StepCheckpoint::Step => "step".to_string(),
        StepCheckpoint::SuccessCriterion { index } => format!("criterion:{index:08}"),
        StepCheckpoint::OnSuccessAction { index } => format!("on-success:{index:08}"),
        StepCheckpoint::OnSuccessCriterion {
            action_index,
            criterion_index,
        } => format!("on-success-criterion:{action_index:08}:{criterion_index:08}"),
        StepCheckpoint::OnFailureAction { index } => format!("on-failure:{index:08}"),
        StepCheckpoint::OnFailureCriterion {
            action_index,
            criterion_index,
        } => format!("on-failure-criterion:{action_index:08}:{criterion_index:08}"),
        StepCheckpoint::OnSuccessRetrySelected { action_index } => {
            format!("on-success-retry-selected:{action_index:08}")
        }
        StepCheckpoint::OnSuccessRetryDelay { action_index } => {
            format!("on-success-retry-delay:{action_index:08}")
        }
        StepCheckpoint::OnFailureRetrySelected { action_index } => {
            format!("on-failure-retry-selected:{action_index:08}")
        }
        StepCheckpoint::OnFailureRetryDelay { action_index } => {
            format!("on-failure-retry-delay:{action_index:08}")
        }
        StepCheckpoint::Output { name } => format!("output:{name}"),
        _ => "step".to_string(),
    }
}

fn retry_lifecycle_action_checkpoint(checkpoint: &StepCheckpoint) -> Option<StepCheckpoint> {
    match checkpoint {
        StepCheckpoint::OnSuccessRetrySelected { action_index }
        | StepCheckpoint::OnSuccessRetryDelay { action_index } => {
            Some(StepCheckpoint::OnSuccessAction {
                index: *action_index,
            })
        }
        StepCheckpoint::OnFailureRetrySelected { action_index }
        | StepCheckpoint::OnFailureRetryDelay { action_index } => {
            Some(StepCheckpoint::OnFailureAction {
                index: *action_index,
            })
        }
        _ => None,
    }
}

#[derive(Debug, Default)]
struct SourceMetadata {
    checkpoints: Vec<IndexedCheckpoint>,
    line_contexts: BTreeMap<u32, SourceLineContext>,
    output_expressions: BTreeMap<(String, String, String), String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ActionSection {
    OnSuccess,
    OnFailure,
}

/// Position in the YAML tree during event-driven parsing.
enum PathSegment {
    Map {
        active_key: Option<String>,
        key_line: Option<u32>,
        expecting_value: bool,
    },
    Seq {
        index: usize,
    },
}

/// SAX-style receiver that builds [`SourceMetadata`] from yaml-rust2 parse events.
struct MetadataReceiver {
    path: Vec<PathSegment>,
    checkpoints: Vec<IndexedCheckpoint>,
    line_contexts: BTreeMap<u32, SourceLineContext>,
    output_expressions: BTreeMap<(String, String, String), String>,
    current_workflow_id: String,
    current_step_id: String,
    step_mapping_start_line: Option<u32>,
    criterion_index: usize,
    on_success_action_index: usize,
    on_failure_action_index: usize,
    current_action_section: Option<ActionSection>,
    current_action_index: Option<usize>,
    action_criteria_index: usize,
}

impl MetadataReceiver {
    fn new() -> Self {
        Self {
            path: Vec::new(),
            checkpoints: Vec::new(),
            line_contexts: BTreeMap::new(),
            output_expressions: BTreeMap::new(),
            current_workflow_id: String::new(),
            current_step_id: String::new(),
            step_mapping_start_line: None,
            criterion_index: 0,
            on_success_action_index: 0,
            on_failure_action_index: 0,
            current_action_section: None,
            current_action_index: None,
            action_criteria_index: 0,
        }
    }

    fn into_metadata(self) -> SourceMetadata {
        SourceMetadata {
            checkpoints: self.checkpoints,
            line_contexts: self.line_contexts,
            output_expressions: self.output_expressions,
        }
    }

    fn line_from_mark(mark: YamlMarker) -> u32 {
        // yaml-rust2 Marker::line() is 0-based but the mark passed to on_event
        // points to the scanner position after the token, effectively 1-based
        // for our purposes.
        u32::try_from(mark.line()).unwrap_or(u32::MAX)
    }

    fn record_context(&mut self, line: u32, area: BreakpointArea, prefer_forward_snap: bool) {
        if self.current_workflow_id.is_empty() || self.current_step_id.is_empty() {
            return;
        }
        self.line_contexts.insert(
            line,
            SourceLineContext {
                workflow_id: self.current_workflow_id.clone(),
                step_id: self.current_step_id.clone(),
                area,
                prefer_forward_snap,
            },
        );
    }

    fn push_checkpoint(&mut self, line: u32, checkpoint: StepCheckpoint) {
        self.checkpoints.push(IndexedCheckpoint {
            line,
            workflow_id: self.current_workflow_id.clone(),
            step_id: self.current_step_id.clone(),
            checkpoint,
        });
    }

    /// Returns true if we are inside `workflows` → seq → map → `steps` → seq.
    fn is_in_steps_seq(&self) -> bool {
        // Pattern: Map(workflows) / Seq / Map(active_key=steps) / Seq
        let len = self.path.len();
        if len < 4 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Seq { .. })
            && self.parent_key_at(len - 2) == Some("steps")
    }

    /// Returns true if we are inside a step mapping (child of steps seq).
    fn is_in_step_mapping(&self) -> bool {
        // Pattern: Map(workflows) / Seq / Map(active_key=steps) / Seq / Map(step)
        let len = self.path.len();
        if len < 5 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Map { .. })
            && matches!(self.path[len - 2], PathSegment::Seq { .. })
            && self.parent_key_at(len - 3) == Some("steps")
    }

    /// Returns the active key of the map at `path[index]`, if it is a map.
    fn parent_key_at(&self, index: usize) -> Option<&str> {
        match self.path.get(index) {
            Some(PathSegment::Map {
                active_key: Some(key),
                ..
            }) => Some(key.as_str()),
            _ => None,
        }
    }

    /// Returns the section key of the innermost step-level section we are inside.
    fn step_section_key(&self) -> Option<&str> {
        // Walk from the top of the stack looking for a map whose active key
        // is one of the recognized section headers.
        for segment in self.path.iter().rev() {
            if let PathSegment::Map {
                active_key: Some(key),
                ..
            } = segment
            {
                match key.as_str() {
                    "successCriteria" | "onSuccess" | "onFailure" | "criteria" | "outputs" => {
                        return Some(key.as_str());
                    }
                    _ => {}
                }
            }
        }
        None
    }

    /// The current breakpoint area derived from the path stack.
    fn current_area(&self) -> BreakpointArea {
        match self.step_section_key() {
            Some("outputs") => BreakpointArea::Outputs,
            Some("criteria") => match self.current_action_section {
                Some(ActionSection::OnSuccess) => BreakpointArea::OnSuccess,
                Some(ActionSection::OnFailure) => BreakpointArea::OnFailure,
                None => BreakpointArea::Step,
            },
            Some("onSuccess") => BreakpointArea::OnSuccess,
            Some("onFailure") => BreakpointArea::OnFailure,
            Some("successCriteria") => BreakpointArea::SuccessCriteria,
            _ => BreakpointArea::Step,
        }
    }

    /// Is the innermost seq a `successCriteria` list?
    fn is_in_success_criteria_seq(&self) -> bool {
        let len = self.path.len();
        if len < 2 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Seq { .. })
            && self.parent_key_at(len - 2) == Some("successCriteria")
    }

    /// Is the innermost seq an `onSuccess` list?
    fn is_in_on_success_seq(&self) -> bool {
        let len = self.path.len();
        if len < 2 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Seq { .. })
            && self.parent_key_at(len - 2) == Some("onSuccess")
    }

    /// Is the innermost seq an `onFailure` list?
    fn is_in_on_failure_seq(&self) -> bool {
        let len = self.path.len();
        if len < 2 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Seq { .. })
            && self.parent_key_at(len - 2) == Some("onFailure")
    }

    /// Is the innermost seq a `criteria` list inside an action?
    fn is_in_action_criteria_seq(&self) -> bool {
        let len = self.path.len();
        if len < 2 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Seq { .. })
            && self.parent_key_at(len - 2) == Some("criteria")
    }

    /// Is the innermost map an `outputs` map?
    fn is_in_outputs_map(&self) -> bool {
        let len = self.path.len();
        if len < 2 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Map { .. })
            && self.parent_key_at(len - 2) == Some("outputs")
    }

    /// After a value is consumed (MappingEnd, SequenceEnd, Alias, or value scalar),
    /// reset the parent state so it's ready for the next key or seq item.
    fn consume_value_in_parent(&mut self) {
        let Some(parent) = self.path.last_mut() else {
            return;
        };
        match parent {
            PathSegment::Map {
                active_key,
                key_line,
                expecting_value,
            } => {
                *active_key = None;
                *key_line = None;
                *expecting_value = false;
            }
            PathSegment::Seq { index } => {
                *index = index.saturating_add(1);
            }
        }
    }

    fn handle_mapping_start(&mut self, mark: YamlMarker) {
        let line = Self::line_from_mark(mark);

        // If parent is the steps seq, this is a new step mapping.
        if self.is_in_steps_seq() {
            self.step_mapping_start_line = Some(line);
            self.current_step_id.clear();
            self.criterion_index = 0;
            self.on_success_action_index = 0;
            self.on_failure_action_index = 0;
            self.current_action_section = None;
            self.current_action_index = None;
            self.action_criteria_index = 0;
        }

        // If parent is an onSuccess or onFailure seq, emit the action checkpoint.
        if self.is_in_on_success_seq() {
            let action_index = self.on_success_action_index;
            self.on_success_action_index = self.on_success_action_index.saturating_add(1);
            self.current_action_section = Some(ActionSection::OnSuccess);
            self.current_action_index = Some(action_index);
            self.action_criteria_index = 0;
            self.push_checkpoint(
                line,
                StepCheckpoint::OnSuccessAction {
                    index: action_index,
                },
            );
        } else if self.is_in_on_failure_seq() {
            let action_index = self.on_failure_action_index;
            self.on_failure_action_index = self.on_failure_action_index.saturating_add(1);
            self.current_action_section = Some(ActionSection::OnFailure);
            self.current_action_index = Some(action_index);
            self.action_criteria_index = 0;
            self.push_checkpoint(
                line,
                StepCheckpoint::OnFailureAction {
                    index: action_index,
                },
            );
        } else if self.is_in_success_criteria_seq() {
            // Criterion item is a mapping (e.g. `- condition: ...`).
            self.push_checkpoint(
                line,
                StepCheckpoint::SuccessCriterion {
                    index: self.criterion_index,
                },
            );
            self.criterion_index = self.criterion_index.saturating_add(1);
        } else if self.is_in_action_criteria_seq() {
            let checkpoint = match self.current_action_section {
                Some(ActionSection::OnSuccess) => StepCheckpoint::OnSuccessCriterion {
                    action_index: self.current_action_index.unwrap_or(0),
                    criterion_index: self.action_criteria_index,
                },
                Some(ActionSection::OnFailure) => StepCheckpoint::OnFailureCriterion {
                    action_index: self.current_action_index.unwrap_or(0),
                    criterion_index: self.action_criteria_index,
                },
                None => StepCheckpoint::Step,
            };
            self.push_checkpoint(line, checkpoint);
            self.action_criteria_index = self.action_criteria_index.saturating_add(1);
        }

        self.path.push(PathSegment::Map {
            active_key: None,
            key_line: None,
            expecting_value: false,
        });
    }

    fn handle_mapping_end(&mut self) {
        // If leaving a step mapping, clear step context.
        if self.is_in_step_mapping() {
            self.current_step_id.clear();
            self.step_mapping_start_line = None;
        }

        // If leaving a workflow mapping, clear workflow context.
        // Pattern: workflows / seq / map(workflow) — path len would be the workflow map level.
        let len = self.path.len();
        if len >= 3
            && matches!(self.path[len - 1], PathSegment::Map { .. })
            && matches!(self.path[len - 2], PathSegment::Seq { .. })
            && self.parent_key_at(len - 3) == Some("workflows")
        {
            self.current_workflow_id.clear();
        }

        self.path.pop();
        self.consume_value_in_parent();
    }

    fn handle_sequence_start(&mut self, mark: YamlMarker) {
        let line = Self::line_from_mark(mark);

        // If parent key is a section header, record line_context with forward snap.
        if let Some(PathSegment::Map {
            active_key: Some(key),
            key_line,
            ..
        }) = self.path.last()
        {
            let ctx_line = key_line.unwrap_or(line);
            match key.as_str() {
                "successCriteria" => {
                    self.record_context(ctx_line, BreakpointArea::SuccessCriteria, true);
                    self.criterion_index = 0;
                }
                "onSuccess" => {
                    self.record_context(ctx_line, BreakpointArea::OnSuccess, true);
                    self.on_success_action_index = 0;
                }
                "onFailure" => {
                    self.record_context(ctx_line, BreakpointArea::OnFailure, true);
                    self.on_failure_action_index = 0;
                }
                "criteria" => {
                    let area = match self.current_action_section {
                        Some(ActionSection::OnSuccess) => BreakpointArea::OnSuccess,
                        Some(ActionSection::OnFailure) => BreakpointArea::OnFailure,
                        None => BreakpointArea::Step,
                    };
                    self.record_context(ctx_line, area, true);
                    self.action_criteria_index = 0;
                }
                _ => {}
            }
        }

        self.path.push(PathSegment::Seq { index: 0 });
    }

    fn handle_sequence_end(&mut self) {
        self.path.pop();
        self.consume_value_in_parent();
    }

    fn handle_scalar(&mut self, value: String, mark: YamlMarker) {
        let line = Self::line_from_mark(mark);

        // Snapshot the parent state to avoid holding a mutable borrow on self.path
        // while calling other &self / &mut self methods.
        enum ParentKind {
            MapKey,
            MapValue {
                key_name: String,
                key_mark_line: u32,
            },
            Seq,
        }

        let parent_kind = match self.path.last() {
            Some(PathSegment::Map {
                expecting_value,
                active_key,
                key_line,
                ..
            }) => {
                if *expecting_value {
                    ParentKind::MapValue {
                        key_name: active_key.clone().unwrap_or_default(),
                        key_mark_line: key_line.unwrap_or(line),
                    }
                } else {
                    ParentKind::MapKey
                }
            }
            Some(PathSegment::Seq { .. }) => ParentKind::Seq,
            None => return,
        };

        match parent_kind {
            ParentKind::MapValue {
                key_name,
                key_mark_line,
            } => {
                if self.is_in_outputs_map() {
                    self.push_checkpoint(
                        key_mark_line,
                        StepCheckpoint::Output {
                            name: key_name.clone(),
                        },
                    );
                    self.output_expressions.insert(
                        (
                            self.current_workflow_id.clone(),
                            self.current_step_id.clone(),
                            key_name,
                        ),
                        value,
                    );
                } else if key_name == "workflowId" && self.is_in_workflow_mapping() {
                    self.current_workflow_id = value;
                } else if key_name == "stepId" && self.is_in_step_mapping() {
                    self.current_step_id = value;
                    if let Some(step_line) = self.step_mapping_start_line {
                        self.push_checkpoint(step_line, StepCheckpoint::Step);
                    }
                } else {
                    let area = self.current_area();
                    self.record_context(line, area, false);
                }

                // Reset parent for next key.
                if let Some(PathSegment::Map {
                    active_key,
                    key_line,
                    expecting_value,
                }) = self.path.last_mut()
                {
                    *active_key = None;
                    *key_line = None;
                    *expecting_value = false;
                }
            }
            ParentKind::MapKey => {
                // Set key on parent.
                if let Some(PathSegment::Map {
                    active_key,
                    key_line,
                    expecting_value,
                }) = self.path.last_mut()
                {
                    *active_key = Some(value.clone());
                    *key_line = Some(line);
                    *expecting_value = true;
                }

                if value == "outputs" && self.is_in_step_mapping_via_parent() {
                    self.record_context(line, BreakpointArea::Outputs, true);
                }
            }
            ParentKind::Seq => {
                if self.is_in_success_criteria_seq() {
                    self.push_checkpoint(
                        line,
                        StepCheckpoint::SuccessCriterion {
                            index: self.criterion_index,
                        },
                    );
                    self.criterion_index = self.criterion_index.saturating_add(1);
                } else if self.is_in_action_criteria_seq() {
                    let checkpoint = match self.current_action_section {
                        Some(ActionSection::OnSuccess) => StepCheckpoint::OnSuccessCriterion {
                            action_index: self.current_action_index.unwrap_or(0),
                            criterion_index: self.action_criteria_index,
                        },
                        Some(ActionSection::OnFailure) => StepCheckpoint::OnFailureCriterion {
                            action_index: self.current_action_index.unwrap_or(0),
                            criterion_index: self.action_criteria_index,
                        },
                        None => StepCheckpoint::Step,
                    };
                    self.push_checkpoint(line, checkpoint);
                    self.action_criteria_index = self.action_criteria_index.saturating_add(1);
                } else if self.is_in_on_success_seq() {
                    let action_index = self.on_success_action_index;
                    self.on_success_action_index = self.on_success_action_index.saturating_add(1);
                    self.push_checkpoint(
                        line,
                        StepCheckpoint::OnSuccessAction {
                            index: action_index,
                        },
                    );
                } else if self.is_in_on_failure_seq() {
                    let action_index = self.on_failure_action_index;
                    self.on_failure_action_index = self.on_failure_action_index.saturating_add(1);
                    self.push_checkpoint(
                        line,
                        StepCheckpoint::OnFailureAction {
                            index: action_index,
                        },
                    );
                }

                // Increment seq index.
                if let Some(PathSegment::Seq { index }) = self.path.last_mut() {
                    *index = index.saturating_add(1);
                }
            }
        }
    }

    /// True if current top-of-stack is inside a workflow mapping (not step).
    fn is_in_workflow_mapping(&self) -> bool {
        let len = self.path.len();
        if len < 3 {
            return false;
        }
        matches!(self.path[len - 1], PathSegment::Map { .. })
            && matches!(self.path[len - 2], PathSegment::Seq { .. })
            && self.parent_key_at(len - 3) == Some("workflows")
    }

    /// True if the parent of the current map is a step mapping.
    /// Used when we're inside a nested map (like outputs) but need to know we're
    /// within a step context.
    fn is_in_step_mapping_via_parent(&self) -> bool {
        // We need to find a step mapping ancestor.
        let len = self.path.len();
        if len < 5 {
            return false;
        }
        // Look for the steps/seq/map pattern in the path.
        for i in 0..len.saturating_sub(2) {
            if self.parent_key_at(i) == Some("steps")
                && matches!(self.path.get(i + 1), Some(PathSegment::Seq { .. }))
                && matches!(self.path.get(i + 2), Some(PathSegment::Map { .. }))
            {
                return true;
            }
        }
        false
    }

    fn handle_alias(&mut self) {
        self.consume_value_in_parent();
    }
}

impl MarkedEventReceiver for MetadataReceiver {
    fn on_event(&mut self, event: YamlEvent, mark: YamlMarker) {
        match event {
            YamlEvent::MappingStart(_, _) => self.handle_mapping_start(mark),
            YamlEvent::MappingEnd => self.handle_mapping_end(),
            YamlEvent::SequenceStart(_, _) => self.handle_sequence_start(mark),
            YamlEvent::SequenceEnd => self.handle_sequence_end(),
            YamlEvent::Scalar(value, _, _, _) => self.handle_scalar(value, mark),
            YamlEvent::Alias(_) => self.handle_alias(),
            _ => {}
        }
    }
}

fn extract_source_metadata(text: &str) -> SourceMetadata {
    let mut receiver = MetadataReceiver::new();
    let mut parser = YamlParser::new_from_str(text);
    // Parsing failures degrade to empty metadata rather than blocking DAP.
    let _ = parser.load(&mut receiver, false);
    receiver.into_metadata()
}

#[cfg(test)]
fn extract_checkpoints_from_text(text: &str) -> Vec<IndexedCheckpoint> {
    extract_source_metadata(text).checkpoints
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_checkpoints_from_text_includes_action_and_output_lines() {
        let text = r#"
workflows:
  - workflowId: get-hackernews
    steps:
      - stepId: fetch-rss
        operationPath: https://hnrss.org/frontpage
        successCriteria:
          - condition: $statusCode == 200
        onSuccess:
          - type: goto
            stepId: done
            criteria:
              - condition: $statusCode == 200
        onFailure:
          - type: retry
            criteria:
              - condition: $statusCode == 503
          - type: end
        outputs:
          title_1: //item[1]/title
          link_1: //item[1]/link
"#;
        let checkpoints = extract_checkpoints_from_text(text);
        assert!(checkpoints.iter().any(|entry| {
            entry.line == 5
                && matches!(entry.checkpoint, StepCheckpoint::Step)
                && entry.workflow_id == "get-hackernews"
                && entry.step_id == "fetch-rss"
        }));
        assert!(checkpoints.iter().any(|entry| {
            entry.line == 8
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::SuccessCriterion { index: 0 }
                )
        }));
        assert!(checkpoints.iter().any(|entry| {
            matches!(
                entry.checkpoint,
                StepCheckpoint::OnSuccessAction { index: 0 }
            )
        }));
        assert!(checkpoints.iter().any(|entry| {
            matches!(
                entry.checkpoint,
                StepCheckpoint::OnSuccessCriterion {
                    action_index: 0,
                    criterion_index: 0
                }
            )
        }));
        assert!(checkpoints.iter().any(|entry| {
            matches!(
                entry.checkpoint,
                StepCheckpoint::OnFailureAction { index: 0 }
            )
        }));
        assert!(checkpoints.iter().any(|entry| {
            matches!(
                entry.checkpoint,
                StepCheckpoint::OnFailureCriterion {
                    action_index: 0,
                    criterion_index: 0
                }
            )
        }));
        assert!(checkpoints.iter().any(|entry| {
            matches!(
                entry.checkpoint,
                StepCheckpoint::OnFailureAction { index: 1 }
            )
        }));
        assert!(checkpoints.iter().any(|entry| {
            entry.line == 20
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::Output { ref name } if name == "title_1"
                )
        }));
        assert!(checkpoints.iter().any(|entry| {
            entry.line == 21
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::Output { ref name } if name == "link_1"
                )
        }));
    }

    #[test]
    fn extract_source_metadata_tracks_output_expressions() {
        let text = r#"
workflows:
  - workflowId: get-hackernews
    steps:
      - stepId: fetch-rss
        outputs:
          title_1: //item[1]/title
"#;
        let metadata = extract_source_metadata(text);
        let key = (
            "get-hackernews".to_string(),
            "fetch-rss".to_string(),
            "title_1".to_string(),
        );
        assert_eq!(
            metadata.output_expressions.get(&key).map(String::as_str),
            Some("//item[1]/title")
        );
    }

    #[test]
    fn resolve_breakpoint_checkpoint_snaps_on_failure_header_to_failure_action() {
        let text = r#"
workflows:
  - workflowId: wf
    steps:
      - stepId: fetch
        successCriteria:
          - condition: $statusCode == 200
        onFailure:
          - type: retry
            criteria:
              - condition: $statusCode == 503
          - type: end
"#;
        let metadata = extract_source_metadata(text);
        let index = SourceIndex {
            path: "/tmp/workflow.arazzo.yaml".to_string(),
            checkpoints: metadata.checkpoints,
            line_contexts: metadata.line_contexts,
            output_expressions: metadata.output_expressions,
        };
        let on_failure_line = u32::try_from(
            text.lines()
                .position(|line| line.trim() == "onFailure:")
                .unwrap_or(0)
                .saturating_add(1),
        )
        .unwrap_or(0);
        let resolved = resolve_breakpoint_checkpoint(on_failure_line, &index, Some("wf"));
        let resolved = match resolved {
            Some(value) => value,
            None => panic!("expected onFailure header to resolve to failure action"),
        };
        assert!(resolved.line > on_failure_line);
        assert!(matches!(
            resolved.checkpoint,
            StepCheckpoint::OnFailureAction { index: 0 }
        ));
    }

    #[test]
    fn resolve_source_breakpoints_reports_mapped_checkpoint_name() {
        let text = r#"
workflows:
  - workflowId: wf
    steps:
      - stepId: fetch
        successCriteria:
          - condition: $statusCode == 200
        onFailure:
          - type: end
"#;
        let metadata = extract_source_metadata(text);
        let source_path = "/tmp/workflow.arazzo.yaml".to_string();
        let index = SourceIndex {
            path: source_path.clone(),
            checkpoints: metadata.checkpoints,
            line_contexts: metadata.line_contexts,
            output_expressions: metadata.output_expressions,
        };

        let on_failure_line = u32::try_from(
            text.lines()
                .position(|line| line.trim() == "onFailure:")
                .unwrap_or(0)
                .saturating_add(1),
        )
        .unwrap_or(0);

        let resolved = resolve_source_breakpoints(
            &source_path,
            &[DapBreakpoint {
                line: on_failure_line,
                condition: None,
            }],
            Some("wf"),
            Some(&index),
        );
        assert_eq!(resolved.resolved.len(), 1);
        let mapped = &resolved.resolved[0];
        assert!(mapped.verified);
        let message = mapped.message.as_deref().unwrap_or("");
        assert!(message.contains("onFailure[0]"));
        assert!(message.contains("mapped line"));
    }

    #[test]
    fn extract_source_metadata_handles_flow_style_outputs() {
        let text = r#"
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        outputs: {title: "$response.body.title", count: "$response.body.count"}
"#;
        let metadata = extract_source_metadata(text);
        assert!(metadata.checkpoints.iter().any(|entry| {
            matches!(
                &entry.checkpoint,
                StepCheckpoint::Output { name } if name == "title"
            )
        }));
        assert!(metadata.checkpoints.iter().any(|entry| {
            matches!(
                &entry.checkpoint,
                StepCheckpoint::Output { name } if name == "count"
            )
        }));
        let key = ("wf".to_string(), "s1".to_string(), "title".to_string());
        assert_eq!(
            metadata.output_expressions.get(&key).map(String::as_str),
            Some("$response.body.title")
        );
    }

    #[test]
    fn extract_source_metadata_handles_block_scalar() {
        let text = r#"
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        description: |
          This step fetches data
          from the API endpoint.
        successCriteria:
          - condition: $statusCode == 200
"#;
        let metadata = extract_source_metadata(text);
        assert!(metadata.checkpoints.iter().any(|entry| {
            entry.step_id == "s1" && matches!(entry.checkpoint, StepCheckpoint::Step)
        }));
        assert!(metadata.checkpoints.iter().any(|entry| {
            entry.step_id == "s1"
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::SuccessCriterion { index: 0 }
                )
        }));
    }

    #[test]
    fn extract_source_metadata_handles_flow_sequence_criteria() {
        let text = r#"
workflows:
  - workflowId: wf
    steps:
      - stepId: s1
        successCriteria: [{condition: "$statusCode == 200"}]
"#;
        let metadata = extract_source_metadata(text);
        assert!(metadata.checkpoints.iter().any(|entry| {
            entry.step_id == "s1" && matches!(entry.checkpoint, StepCheckpoint::Step)
        }));
        assert!(metadata.checkpoints.iter().any(|entry| {
            entry.step_id == "s1"
                && matches!(
                    entry.checkpoint,
                    StepCheckpoint::SuccessCriterion { index: 0 }
                )
        }));
    }
}
