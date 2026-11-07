//! Values retained by a native task at each suspension boundary.
//!
//! This is a compiler analysis, not a second ownership checker. Core's
//! verifier establishes legal uses and capability disposition first.

use crate::core::{
    self, BlockId, CallArg, Function, FunctionId, Operation, PlaceId, Program, Terminator, ValueId,
};
use std::fmt::Write;
use std::ops::Range;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SuspensionPoint {
    pub block: BlockId,
    /// `None` means the block terminator; `Some(i)` means a pending call at
    /// instruction `i`, after the callee's frame has taken its arguments.
    pub instruction: Option<usize>,
    pub values: Vec<ValueId>,
    pub places: Vec<PlaceId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionFrame {
    pub function: FunctionId,
    pub points: Vec<SuspensionPoint>,
    /// Union of all point-local spill sets. An empty set needs no value area.
    pub value_slots: Vec<ValueId>,
    pub place_slots: Vec<PlaceId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WordSlot<T> {
    pub id: T,
    pub words: Range<usize>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ResumeSlot {
    /// Zero is reserved for first entry; retries start at one.
    pub pc: i64,
    pub block: BlockId,
    pub instruction: Option<usize>,
    pub values: Vec<ValueId>,
    pub places: Vec<PlaceId>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameWords {
    pub function: FunctionId,
    /// Word 0 is the program counter; this range starts at word 1.
    pub result: Range<usize>,
    /// All entry parameters have an initial home. A parameter that also
    /// crosses a suspension shares its retained value slot.
    pub entry_params: Vec<WordSlot<ValueId>>,
    pub values: Vec<WordSlot<ValueId>>,
    pub places: Vec<WordSlot<PlaceId>>,
    pub resumes: Vec<ResumeSlot>,
    pub total_words: usize,
}

/// Assign physical word ranges using one backend-supplied ABI shape width.
/// Every component occupies one stable word, including a widened Bool.
pub fn word_layout(
    program: &Program,
    frame: &FunctionFrame,
    width: impl Fn(core::Type) -> usize,
) -> Result<FrameWords, String> {
    let function = program
        .functions
        .get(frame.function.0)
        .ok_or("invalid frame function")?;
    let mut next = 1usize;
    let mut reserve = |ty| -> Result<Range<usize>, String> {
        let count = width(ty);
        if count == 0 {
            return Err("zero-width native frame value".into());
        }
        let end = next
            .checked_add(count)
            .ok_or("native frame size overflow")?;
        let words = next..end;
        next = end;
        Ok(words)
    };
    let result = reserve(function.return_type)?;
    let values = frame
        .value_slots
        .iter()
        .map(|id| {
            Ok(WordSlot {
                id: *id,
                words: reserve(
                    *function
                        .value_types
                        .get(id.0)
                        .ok_or("invalid frame value slot")?,
                )?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let entry_params = function
        .blocks
        .first()
        .ok_or("frame function has no entry block")?
        .params
        .iter()
        .map(|id| {
            let words = match values.iter().find(|slot| slot.id == *id) {
                Some(slot) => slot.words.clone(),
                None => reserve(
                    *function
                        .value_types
                        .get(id.0)
                        .ok_or("invalid frame entry parameter")?,
                )?,
            };
            Ok(WordSlot { id: *id, words })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let places = frame
        .place_slots
        .iter()
        .map(|id| {
            Ok(WordSlot {
                id: *id,
                words: reserve(
                    *function
                        .place_types
                        .get(id.0)
                        .ok_or("invalid frame place slot")?,
                )?,
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    if next
        .checked_mul(std::mem::size_of::<i64>())
        .is_none_or(|bytes| bytes > i32::MAX as usize)
    {
        return Err("native frame exceeds addressable fixed offsets".into());
    }
    let resumes = frame
        .points
        .iter()
        .enumerate()
        .map(|(index, point)| {
            Ok(ResumeSlot {
                pc: i64::try_from(index.checked_add(1).ok_or("native resume count overflow")?)
                    .map_err(|_| "native resume count overflow")?,
                block: point.block,
                instruction: point.instruction,
                values: point.values.clone(),
                places: point.places.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(FrameWords {
        function: frame.function,
        result,
        entry_params,
        values,
        places,
        resumes,
        total_words: next,
    })
}

#[derive(Clone, PartialEq, Eq)]
struct Live {
    values: Vec<bool>,
    places: Vec<bool>,
}

impl Live {
    fn empty(function: &Function) -> Self {
        Self {
            values: vec![false; function.value_types.len()],
            places: vec![false; function.place_types.len()],
        }
    }

    fn union(&mut self, other: &Self) {
        for (here, there) in self.values.iter_mut().zip(&other.values) {
            *here |= *there;
        }
        for (here, there) in self.places.iter_mut().zip(&other.places) {
            *here |= *there;
        }
    }

    fn values(&self) -> Vec<ValueId> {
        self.values
            .iter()
            .enumerate()
            .filter_map(|(id, live)| live.then_some(ValueId(id)))
            .collect()
    }

    fn places(&self) -> Vec<PlaceId> {
        self.places
            .iter()
            .enumerate()
            .filter_map(|(id, live)| live.then_some(PlaceId(id)))
            .collect()
    }
}

/// A synchronous caller can park only if a callee can park. Spawn starts a
/// separate task, so it does not make the parent suspendable by itself.
pub fn suspendable_functions(program: &Program) -> Result<Vec<bool>, String> {
    core::verify(program)?;
    let mut suspendable = program
        .functions
        .iter()
        .map(|function| {
            function.blocks.iter().any(|block| {
                matches!(
                    block.terminator,
                    Terminator::ChannelSend { .. } | Terminator::ChannelReceive { .. }
                )
            })
        })
        .collect::<Vec<_>>();
    loop {
        let mut changed = false;
        for (id, function) in program.functions.iter().enumerate() {
            if suspendable[id] {
                continue;
            }
            if function.blocks.iter().flat_map(|block| &block.instructions).any(|instruction| {
                matches!(&instruction.operation, Operation::Call(target, _) if suspendable[target.0])
            }) {
                suspendable[id] = true;
                changed = true;
            }
        }
        if !changed {
            return Ok(suspendable);
        }
    }
}

/// Derive the point-local retained state for each function that can park.
/// No frame is assigned to functions with no suspension point.
pub fn analyze(program: &Program) -> Result<Vec<FunctionFrame>, String> {
    let suspendable = suspendable_functions(program)?;
    let mut frames = Vec::new();
    for (function_id, function) in program.functions.iter().enumerate() {
        if !suspendable[function_id] {
            continue;
        }
        let mut incoming = vec![Live::empty(function); function.blocks.len()];
        loop {
            let mut changed = false;
            for block_id in (0..function.blocks.len()).rev() {
                let mut live = terminator_live(function, block_id, &incoming);
                for instruction in function.blocks[block_id].instructions.iter().rev() {
                    instruction_live(&mut live, &instruction.operation, instruction.result);
                }
                if incoming[block_id] != live {
                    incoming[block_id] = live;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let mut points = Vec::new();
        let mut all = Live::empty(function);
        for (block_id, block) in function.blocks.iter().enumerate() {
            let mut live = terminator_live(function, block_id, &incoming);
            if matches!(
                block.terminator,
                Terminator::ChannelSend { .. } | Terminator::ChannelReceive { .. }
            ) {
                all.union(&live);
                points.push(point(BlockId(block_id), None, &live));
            }
            for (instruction_id, instruction) in block.instructions.iter().enumerate().rev() {
                if let Operation::Call(target, args) = &instruction.operation
                    && suspendable[target.0]
                {
                    let mut retained = live.clone();
                    if let Some(result) = instruction.result {
                        retained.values[result.0] = false;
                    }
                    for arg in args {
                        if let CallArg::Borrow(place, _) = arg {
                            retained.places[place.0] = true;
                        }
                    }
                    all.union(&retained);
                    points.push(point(BlockId(block_id), Some(instruction_id), &retained));
                }
                instruction_live(&mut live, &instruction.operation, instruction.result);
            }
        }
        points.sort_by_key(|point| (point.block.0, point.instruction.unwrap_or(usize::MAX)));
        frames.push(FunctionFrame {
            function: FunctionId(function_id),
            points,
            value_slots: all.values(),
            place_slots: all.places(),
        });
    }
    Ok(frames)
}

/// Stable textual view of the planned frame payload and resume points.
pub fn format(program: &Program) -> Result<String, String> {
    let frames = analyze(program)?;
    let mut output = String::new();
    if frames.is_empty() {
        return Ok("no retained frames\n".into());
    }
    for frame in frames {
        let function = &program.functions[frame.function.0];
        writeln!(output, "frame {} {{", function.name).map_err(|error| error.to_string())?;
        for value in &frame.value_slots {
            writeln!(
                output,
                "  v{}: {:?}",
                value.0, function.value_types[value.0]
            )
            .map_err(|error| error.to_string())?;
        }
        for place in &frame.place_slots {
            writeln!(
                output,
                "  p{}: {:?}",
                place.0, function.place_types[place.0]
            )
            .map_err(|error| error.to_string())?;
        }
        for point in &frame.points {
            let location = point.instruction.map_or_else(
                || "terminator".into(),
                |index| format!("instruction{index}"),
            );
            write!(
                output,
                "  resume block{} {}: values [",
                point.block.0, location
            )
            .map_err(|error| error.to_string())?;
            for (index, value) in point.values.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                write!(output, "v{}", value.0).map_err(|error| error.to_string())?;
            }
            output.push_str("] places [");
            for (index, place) in point.places.iter().enumerate() {
                if index > 0 {
                    output.push_str(", ");
                }
                write!(output, "p{}", place.0).map_err(|error| error.to_string())?;
            }
            output.push_str("]\n");
        }
        output.push_str("}\n");
    }
    Ok(output)
}

fn point(block: BlockId, instruction: Option<usize>, live: &Live) -> SuspensionPoint {
    SuspensionPoint {
        block,
        instruction,
        values: live.values(),
        places: live.places(),
    }
}

fn instruction_live(live: &mut Live, operation: &Operation, result: Option<ValueId>) {
    if let Some(result) = result {
        live.values[result.0] = false;
    }
    match operation {
        Operation::PlaceInit(place, _) | Operation::Store(place, _) | Operation::Take(place) => {
            // A store replaces the prior value; a take consumes it.
            live.places[place.0] = matches!(operation, Operation::Take(_));
        }
        Operation::Load(place) | Operation::CopyPlace(place) | Operation::ReadField(place, _) => {
            live.places[place.0] = true;
        }
        Operation::Call(_, args) => {
            for arg in args {
                if let CallArg::Borrow(place, _) = arg {
                    live.places[place.0] = true;
                }
            }
        }
        _ => {}
    }
    for value in core::operands(operation) {
        live.values[value.0] = true;
    }
}

fn terminator_live(function: &Function, block_id: usize, incoming: &[Live]) -> Live {
    let mut live = Live::empty(function);
    let terminator = &function.blocks[block_id].terminator;
    let mut edge = |target: BlockId, args: Option<&[ValueId]>| {
        let mut after = incoming[target.0].clone();
        for (index, param) in function.blocks[target.0].params.iter().enumerate() {
            let needed = after.values[param.0];
            after.values[param.0] = false;
            if needed && let Some(args) = args {
                after.values[args[index].0] = true;
            }
        }
        live.union(&after);
    };
    match terminator {
        Terminator::Return(value) => live.values[value.0] = true,
        Terminator::Jump(target, args) => edge(*target, Some(args)),
        Terminator::Branch {
            condition,
            then_block,
            else_block,
        } => {
            edge(*then_block, None);
            edge(*else_block, None);
            live.values[condition.0] = true;
        }
        Terminator::Match { subject, arms } => {
            for arm in arms {
                edge(arm.target, None);
            }
            live.values[subject.0] = true;
        }
        Terminator::ChannelNew {
            capacity,
            success,
            failure,
        } => {
            edge(*success, None);
            edge(*failure, None);
            live.values[capacity.0] = true;
        }
        Terminator::ChannelSend {
            sender,
            message,
            sent,
            closed,
        } => {
            edge(*sent, None);
            edge(*closed, None);
            live.values[sender.0] = true;
            live.values[message.0] = true;
        }
        Terminator::ChannelReceive {
            receiver,
            value,
            end,
        } => {
            edge(*value, None);
            edge(*end, None);
            live.values[receiver.0] = true;
        }
        Terminator::Spawn {
            args,
            success,
            failure,
            ..
        } => {
            edge(*success, None);
            edge(*failure, None);
            for arg in args {
                live.values[arg.0] = true;
            }
        }
    }
    live
}
