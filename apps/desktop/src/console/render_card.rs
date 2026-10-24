//! The floor's renderer in small scenes: a machine's exterior in a card (`exterior`) and its flat views (`flat_view`).
//! Both draw through the floor's own renderer (`engine::render`), each on its own slot. The cards and the flat views are
//! pools of slots: iced keeps one pipeline per type, and two primitives on one pipeline overwrite each other's targets
//! within a frame, so none of them shares a slot.

use std::sync::Arc;
use std::time::{Duration, Instant};

use iced::widget::shader::{self, Viewport};
use iced::{Element, Length, Rectangle, mouse, wgpu, window};

use crate::console::engine::fleet::{self, Machine};
use crate::console::engine::kit;
use crate::console::engine::machine_stage::{MachineStage, View};
use crate::console::engine::render;
use crate::console::engine::scene::Frame;
use crate::console::engine::style::{Palette, Style};

/// The machine cards' slots (`slot` wraps at this count).
pub(crate) const CARD_SLOTS: usize = 8;

/// The flat views' slots, a pool after the cards' (`CARD_SLOTS + index % VIEW_SLOTS`): a machine's four views
/// (`View::FLAT`) take one each. They are apart from the cards because a list card and a flat view of the same machine
/// can be on screen at once, and they would overwrite each other on one pipeline.
pub(crate) const VIEW_SLOTS: usize = 4;

/// On a software adapter a card redraws about 30 times a second, for its status lamp and fans; on a GPU it redraws on
/// every window frame. Its pose holds still (the thumbnail's start pose).
const CARD_FRAME: Duration = Duration::from_millis(33);

/// One slot's renderer: the floor's own pipeline, held apart per slot and per pool (iced keeps one pipeline per type).
pub(crate) struct CardPipeline<const N: usize>(render::Pipeline);

impl<const N: usize> shader::Pipeline for CardPipeline<N> {
    fn new(device: &wgpu::Device, queue: &wgpu::Queue, format: wgpu::TextureFormat) -> Self {
        let mut p = render::Pipeline::new(device, queue, format);
        // A card has no page-sized backdrop: its clear is transparent, so the page shows behind it.
        p.clear_alpha = 0.0;
        // A card's corner is empty, so it is the probe's sample (`APZ_CARD_PROBE`).
        p.probe_corner = true;
        CardPipeline(p)
    }
}

/// A card's frame for the GPU: the floor's own primitive, drawn with its slot's pipeline.
#[derive(Debug)]
pub(crate) struct CardPrimitive<const N: usize> {
    frame: Option<Arc<Frame>>,
}

impl<const N: usize> shader::Primitive for CardPrimitive<N> {
    type Pipeline = CardPipeline<N>;

    fn prepare(
        &self,
        p: &mut CardPipeline<N>,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bounds: &Rectangle,
        viewport: &Viewport,
    ) {
        let Some(frame) = &self.frame else { return };
        let inner = render::Primitive {
            frame: Arc::clone(frame),
        };
        let rect = render::Rect {
            x: bounds.x,
            y: bounds.y,
            width: bounds.width,
            height: bounds.height,
        };
        inner.prepare(&mut p.0, device, queue, &rect, viewport.scale_factor());
    }

    fn draw(&self, p: &CardPipeline<N>, pass: &mut wgpu::RenderPass<'_>) -> bool {
        let Some(frame) = &self.frame else {
            return true;
        };
        let inner = render::Primitive {
            frame: Arc::clone(frame),
        };
        inner.draw(&p.0, pass)
    }
}

// ---- A machine's exterior: the machine stage's model in a card --------------------------------

/// A card's state: its machine stage (built on the first redraw, and again when the console's theme turns) with the
/// theme it was built in, and the frame the GPU draws next. The stage steps in `update`, once per redraw; `draw` only
/// hands the frame over.
#[derive(Default)]
pub(crate) struct CardState {
    stage: Option<(MachineStage, Style, bool)>,
    last: Option<Instant>,
    frame: Option<Arc<Frame>>,
}

/// A machine drawn from `view` in a card, on slot `N`: `View::Iso` is the exterior, the others its flat views.
pub(crate) struct Card<const N: usize> {
    machine: Machine,
    view: View,
}

impl<const N: usize, M> shader::Program<M> for Card<N> {
    type State = CardState;
    type Primitive = CardPrimitive<N>;

    fn update(
        &self,
        st: &mut CardState,
        event: &iced::Event,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Option<shader::Action<M>> {
        let iced::Event::Window(window::Event::RedrawRequested(now)) = event else {
            return None;
        };
        let dt = st
            .last
            .map_or(0.0, |t| now.saturating_duration_since(t).as_secs_f32());
        st.last = Some(*now);
        let dark = !crate::theme::is_light();
        let style = crate::theme::machine_style();
        if st
            .stage
            .as_ref()
            .is_none_or(|(_, built_style, built_dark)| (*built_style, *built_dark) != (style, dark))
        {
            let palette = Palette::of(style, dark);
            st.stage = Some((MachineStage::new(&self.machine, &palette)?, style, dark));
        }
        let (stage, _, _) = st.stage.as_mut()?;
        let aspect = (bounds.width / bounds.height.max(1.0)).max(0.1);
        stage.step(dt, aspect, false);
        st.frame = Some(Arc::new(stage.view_frame(self.view, aspect)));
        if crate::console::engine::render::light() {
            Some(shader::Action::request_redraw_at(*now + CARD_FRAME))
        } else {
            Some(shader::Action::request_redraw())
        }
    }

    fn draw(&self, st: &CardState, _cursor: mouse::Cursor, _bounds: Rectangle) -> CardPrimitive<N> {
        CardPrimitive {
            frame: st.frame.clone(),
        }
    }
}

/// The machine a card draws, built from the screen's fields. Returns None when the kit has no model for the machine, the
/// same case in which `MachineStage::new` returns None (checked here, so a card never builds a stage it cannot show).
pub(crate) fn card_machine(kind: &str, vendor: &str, gpus: usize, active: bool) -> Option<Machine> {
    let machine = Machine {
        kind: kind.to_string(),
        vendor: Some(vendor.to_string()),
        status: "online".into(),
        working: u32::from(active),
        gpus: vec![Default::default(); gpus],
        ..Default::default()
    };
    kit::model(fleet::kit_model_of(&machine))?;
    Some(machine)
}

/// A machine drawn from `view` in a card on pool slot `slot`, one of the cards' (0..`CARD_SLOTS`) or the flat views'
/// (`CARD_SLOTS`..`CARD_SLOTS + VIEW_SLOTS`) literal slots. Returns None for any other slot.
fn card_on<'a, M: 'a>(slot: usize, machine: Machine, view: View) -> Option<Element<'a, M>> {
    macro_rules! card {
        ($($n:literal)*) => {
            match slot {
                $($n => shader::Shader::<M, _>::new(Card::<$n> { machine, view }).width(Length::Fixed(160.0)).height(Length::Fixed(120.0)).into(),)*
                _ => return None,
            }
        };
    }
    Some(card!(0 1 2 3 4 5 6 7 8 9 10 11))
}

/// A machine's exterior drawn in a card on card slot `slot` (wraps). Returns None when the kit has no model for the
/// machine (see `card_machine`).
pub(crate) fn exterior<'a, M: 'a>(
    slot: usize,
    kind: &str,
    vendor: &str,
    gpus: usize,
    active: bool,
) -> Option<Element<'a, M>> {
    card_on(
        slot % CARD_SLOTS,
        card_machine(kind, vendor, gpus, active)?,
        View::Iso,
    )
}

/// A machine's flat view `view` (the `index`th of `View::FLAT`) in a card on its own view slot (`index` wraps at
/// `VIEW_SLOTS`). Returns None when the kit has no model for the machine (see `card_machine`).
pub(crate) fn flat_view<'a, M: 'a>(
    index: usize,
    kind: &str,
    vendor: &str,
    gpus: usize,
    active: bool,
    view: View,
) -> Option<Element<'a, M>> {
    card_on(
        CARD_SLOTS + index % VIEW_SLOTS,
        card_machine(kind, vendor, gpus, active)?,
        view,
    )
}
