//! Simple plotting library.

use std::{cell::RefCell, rc::Rc};

use crate::*;
use epaint::ahash::AHashSet;
use epaint::color::Hsva;
use epaint::util::FloatOrd;
use items::PlotItem;
use legend::LegendWidget;
use transform::{PlotBounds, ScreenTransform};

pub use items::{
    Arrows, Bar, BarChart, BoxElem, BoxPlot, BoxSpread, HLine, Line, LineStyle, MarkerShape,
    Orientation, PlotImage, Points, Polygon, Text, VLine, Value, Values,
};
pub use legend::{Corner, Legend};

use self::items::{num_decimals_with_max_digits, HoverConfig};

mod items;
mod legend;
mod transform;

type HoverFormatterFn = dyn Fn(&HoverConfig, &str, &Value) -> String;
type HoverFormatter = Box<HoverFormatterFn>;

type AxisFormatterFn = dyn Fn(f64) -> String;
type AxisFormatter = Option<Box<AxisFormatterFn>>;

// ----------------------------------------------------------------------------

/// Information about the plot that has to persist between frames.
#[cfg_attr(feature = "serde", derive(serde::Deserialize, serde::Serialize))]
#[derive(Clone)]
struct PlotMemory {
    auto_bounds: bool,
    hovered_entry: Option<String>,
    hidden_items: AHashSet<String>,
    min_auto_bounds: PlotBounds,
    last_screen_transform: ScreenTransform,
    /// Allows to remember the first click position when performing a boxed zoom
    last_click_pos_for_zoom: Option<Pos2>,
}

impl PlotMemory {
    pub fn load(ctx: &Context, id: Id) -> Option<Self> {
        ctx.data().get_persisted(id)
    }

    pub fn store(self, ctx: &Context, id: Id) {
        ctx.data().insert_persisted(id, self);
    }
}

// ----------------------------------------------------------------------------

/// Defines how multiple plots share the same range for one or both of their axes. Can be added while building
/// a plot with [`Plot::link_axis`]. Contains an internal state, meaning that this object should be stored by
/// the user between frames.
#[derive(Clone, PartialEq)]
pub struct LinkedAxisGroup {
    pub(crate) link_x: bool,
    pub(crate) link_y: bool,
    pub(crate) bounds: Rc<RefCell<Option<PlotBounds>>>,
}

impl LinkedAxisGroup {
    pub fn new(link_x: bool, link_y: bool) -> Self {
        Self {
            link_x,
            link_y,
            bounds: Rc::new(RefCell::new(None)),
        }
    }

    /// Only link the x-axis.
    pub fn x() -> Self {
        Self::new(true, false)
    }

    /// Only link the y-axis.
    pub fn y() -> Self {
        Self::new(false, true)
    }

    /// Link both axes. Note that this still respects the aspect ratio of the individual plots.
    pub fn both() -> Self {
        Self::new(true, true)
    }

    /// Change whether the x-axis is linked for this group. Using this after plots in this group have been
    /// drawn in this frame already may lead to unexpected results.
    pub fn set_link_x(&mut self, link: bool) {
        self.link_x = link;
    }

    /// Change whether the y-axis is linked for this group. Using this after plots in this group have been
    /// drawn in this frame already may lead to unexpected results.
    pub fn set_link_y(&mut self, link: bool) {
        self.link_y = link;
    }

    fn get(&self) -> Option<PlotBounds> {
        *self.bounds.borrow()
    }

    fn set(&self, bounds: PlotBounds) {
        *self.bounds.borrow_mut() = Some(bounds);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HoverLine {
    None,
    X,
    Y,
    XY,
}

impl HoverLine {
    pub fn all() -> impl Iterator<Item = HoverLine> {
        [HoverLine::None, HoverLine::X, HoverLine::Y, HoverLine::XY]
            .iter()
            .copied()
    }

    pub fn show_x_line(&self) -> bool {
        matches!(self, &HoverLine::X) || matches!(self, &HoverLine::XY)
    }
    pub fn show_y_line(&self) -> bool {
        matches!(self, &HoverLine::Y) || matches!(self, &HoverLine::XY)
    }
}

impl Default for HoverLine {
    fn default() -> Self {
        HoverLine::XY
    }
}

// ----------------------------------------------------------------------------

/// A 2D plot, e.g. a graph of a function.
///
/// `Plot` supports multiple lines and points.
///
/// ```
/// # egui::__run_test_ui(|ui| {
/// use egui::plot::{Line, Plot, Value, Values};
/// let sin = (0..1000).map(|i| {
///     let x = i as f64 * 0.01;
///     Value::new(x, x.sin())
/// });
/// let line = Line::new(Values::from_values_iter(sin));
/// Plot::new("my_plot").view_aspect(2.0).show(ui, |plot_ui| plot_ui.line(line));
/// # });
/// ```
pub struct Plot {
    id_source: Id,

    center_x_axis: bool,
    center_y_axis: bool,
    allow_zoom: bool,
    allow_drag: bool,
    min_auto_bounds: PlotBounds,
    margin_fraction: Vec2,
    allow_boxed_zoom: bool,
    boxed_zoom_pointer_button: PointerButton,
    linked_axes: Option<LinkedAxisGroup>,

    min_size: Vec2,
    width: Option<f32>,
    height: Option<f32>,
    data_aspect: Option<f32>,
    view_aspect: Option<f32>,

    hover_line: HoverLine,
    show_hover_label: bool,
    hover_formatter: HoverFormatter,
    axis_formatters: [AxisFormatter; 2],
    legend_config: Option<Legend>,
    show_background: bool,
    show_axes: [bool; 2],
}

impl Plot {
    /// Give a unique id for each plot within the same `Ui`.
    pub fn new(id_source: impl std::hash::Hash) -> Self {
        Self {
            id_source: Id::new(id_source),

            center_x_axis: false,
            center_y_axis: false,
            allow_zoom: true,
            allow_drag: true,
            min_auto_bounds: PlotBounds::NOTHING,
            margin_fraction: Vec2::splat(0.05),
            allow_boxed_zoom: true,
            boxed_zoom_pointer_button: PointerButton::Secondary,
            linked_axes: None,

            min_size: Vec2::splat(64.0),
            width: None,
            height: None,
            data_aspect: None,
            view_aspect: None,

            hover_line: HoverLine::XY,
            show_hover_label: true,
            hover_formatter: Plot::default_hover_formatter(),

            axis_formatters: [None, None], // [None; 2] requires Copy
            legend_config: None,
            show_background: true,
            show_axes: [true; 2],
        }
    }

    /// width / height ratio of the data.
    /// For instance, it can be useful to set this to `1.0` for when the two axes show the same
    /// unit.
    /// By default the plot window's aspect ratio is used.
    pub fn data_aspect(mut self, data_aspect: f32) -> Self {
        self.data_aspect = Some(data_aspect);
        self
    }

    /// width / height ratio of the plot region.
    /// By default no fixed aspect ratio is set (and width/height will fill the ui it is in).
    pub fn view_aspect(mut self, view_aspect: f32) -> Self {
        self.view_aspect = Some(view_aspect);
        self
    }

    /// Width of plot. By default a plot will fill the ui it is in.
    /// If you set [`Self::view_aspect`], the width can be calculated from the height.
    pub fn width(mut self, width: f32) -> Self {
        self.min_size.x = width;
        self.width = Some(width);
        self
    }

    /// Height of plot. By default a plot will fill the ui it is in.
    /// If you set [`Self::view_aspect`], the height can be calculated from the width.
    pub fn height(mut self, height: f32) -> Self {
        self.min_size.y = height;
        self.height = Some(height);
        self
    }

    /// Minimum size of the plot view.
    pub fn min_size(mut self, min_size: Vec2) -> Self {
        self.min_size = min_size;
        self
    }

    /// Whether to display hover line(s) or not (lines marking cursor location).
    pub fn hover_line(mut self, hover_line: HoverLine) -> Self {
        self.hover_line = hover_line;
        self
    }

    /// Always keep the x-axis centered. Default: `false`.
    pub fn center_x_axis(mut self, on: bool) -> Self {
        self.center_x_axis = on;
        self
    }

    /// Always keep the y-axis centered. Default: `false`.
    pub fn center_y_axis(mut self, on: bool) -> Self {
        self.center_y_axis = on;
        self
    }

    /// Whether to allow zooming in the plot. Default: `true`.
    pub fn allow_zoom(mut self, on: bool) -> Self {
        self.allow_zoom = on;
        self
    }

    /// Whether to allow zooming in the plot by dragging out a box with the secondary mouse button.
    ///
    /// Default: `true`.
    pub fn allow_boxed_zoom(mut self, on: bool) -> Self {
        self.allow_boxed_zoom = on;
        self
    }

    /// Config the button pointer to use for boxed zooming. Default: `Secondary`
    pub fn boxed_zoom_pointer_button(mut self, boxed_zoom_pointer_button: PointerButton) -> Self {
        self.boxed_zoom_pointer_button = boxed_zoom_pointer_button;
        self
    }

    /// Whether to allow dragging in the plot to move the bounds. Default: `true`.
    pub fn allow_drag(mut self, on: bool) -> Self {
        self.allow_drag = on;
        self
    }

    /// Provide a function to customize the on-hovel label for the x and y axis
    ///
    /// ```
    /// # egui::__run_test_ui(|ui| {
    /// use egui::plot::{Line, Plot, Value, Values};
    /// let sin = (0..1000).map(|i| {
    ///     let x = i as f64 * 0.01;
    ///     Value::new(x, x.sin())
    /// });
    /// let line = Line::new(Values::from_values_iter(sin));
    /// Plot::new("my_plot").view_aspect(2.0)
    /// .hover_formatter(|config, name, value| {
    ///     if !name.is_empty() {
    ///         format!("{}: {:.*}%", name, 1, value.y).to_string()
    ///     } else {
    ///         "".to_string()
    ///     }
    /// })
    /// .show(ui, |plot_ui| plot_ui.line(line));
    /// # });
    /// ```
    pub fn hover_formatter<F: 'static + Fn(&HoverConfig, &str, &Value) -> String>(
        mut self,
        hover_formatter: F,
    ) -> Self {
        self.hover_formatter = Box::new(hover_formatter);
        self
    }

    fn default_hover_formatter() -> Box<dyn Fn(&HoverConfig, &str, &Value) -> String> {
        Box::new(|config, name, value| {
            let mut prefix = String::new();

            if !name.is_empty() {
                prefix = format!("{}\n", name);
            }

            let x_decimals = num_decimals_with_max_digits(value.x, 6);
            let y_decimals = num_decimals_with_max_digits(value.y, 6);

            match config.hover_line {
                HoverLine::None => format!(""),
                HoverLine::X => format!("{}x = {:.*}", prefix, x_decimals, value.x),
                HoverLine::Y => format!("{}y = {:.*}", prefix, y_decimals, value.y),
                HoverLine::XY => format!(
                    "{}x = {:.*}\ny = {:.*}",
                    prefix, x_decimals, value.x, y_decimals, value.y
                ),
            }
        })
    }

    /// Whether to show a label when hovering on axis
    pub fn show_hover_label(mut self, show_hover_label: bool) -> Self {
        self.show_hover_label = show_hover_label;
        self
    }

    /// Provide a function to customize the labels for the X axis.
    ///
    /// This is useful for custom input domains, e.g. date/time.
    ///
    /// If axis labels should not appear for certain values or beyond a certain zoom/resolution,
    /// the formatter function can return empty strings. This is also useful if your domain is
    /// discrete (e.g. only full days in a calendar).
    pub fn x_axis_formatter(mut self, func: impl Fn(f64) -> String + 'static) -> Self {
        self.axis_formatters[0] = Some(Box::new(func));
        self
    }

    /// Provide a function to customize the labels for the Y axis.
    ///
    /// This is useful for custom value representation, e.g. percentage or units.
    ///
    /// If axis labels should not appear for certain values or beyond a certain zoom/resolution,
    /// the formatter function can return empty strings. This is also useful if your Y values are
    /// discrete (e.g. only integers).
    pub fn y_axis_formatter(mut self, func: impl Fn(f64) -> String + 'static) -> Self {
        self.axis_formatters[1] = Some(Box::new(func));
        self
    }

    /// Expand bounds to include the given x value.
    /// For instance, to always show the y axis, call `plot.include_x(0.0)`.
    pub fn include_x(mut self, x: impl Into<f64>) -> Self {
        self.min_auto_bounds.extend_with_x(x.into());
        self
    }

    /// Expand bounds to include the given y value.
    /// For instance, to always show the x axis, call `plot.include_y(0.0)`.
    pub fn include_y(mut self, y: impl Into<f64>) -> Self {
        self.min_auto_bounds.extend_with_y(y.into());
        self
    }

    /// Show a legend including all named items.
    pub fn legend(mut self, legend: Legend) -> Self {
        self.legend_config = Some(legend);
        self
    }

    /// Whether or not to show the background `Rect`.
    /// Can be useful to disable if the plot is overlaid over existing content.
    /// Default: `true`.
    pub fn show_background(mut self, show: bool) -> Self {
        self.show_background = show;
        self
    }

    /// Show the axes.
    /// Can be useful to disable if the plot is overlaid over an existing grid or content.
    /// Default: `[true; 2]`.
    pub fn show_axes(mut self, show: [bool; 2]) -> Self {
        self.show_axes = show;
        self
    }

    /// Add a [`LinkedAxisGroup`] so that this plot will share the bounds with other plots that have this
    /// group assigned. A plot cannot belong to more than one group.
    pub fn link_axis(mut self, group: LinkedAxisGroup) -> Self {
        self.linked_axes = Some(group);
        self
    }

    /// Interact with and add items to the plot and finally draw it.
    pub fn show<R>(self, ui: &mut Ui, build_fn: impl FnOnce(&mut PlotUi) -> R) -> InnerResponse<R> {
        let Self {
            id_source,
            center_x_axis,
            center_y_axis,
            allow_zoom,
            allow_drag,
            allow_boxed_zoom,
            boxed_zoom_pointer_button: boxed_zoom_pointer,
            min_auto_bounds,
            margin_fraction,
            width,
            height,
            min_size,
            data_aspect,
            view_aspect,
            mut hover_line,
            show_hover_label,
            hover_formatter,
            axis_formatters,
            legend_config,
            show_background,
            show_axes,
            linked_axes,
        } = self;

        // Determine the size of the plot in the UI
        let size = {
            let width = width
                .unwrap_or_else(|| {
                    if let (Some(height), Some(aspect)) = (height, view_aspect) {
                        height * aspect
                    } else {
                        ui.available_size_before_wrap().x
                    }
                })
                .at_least(min_size.x);

            let height = height
                .unwrap_or_else(|| {
                    if let Some(aspect) = view_aspect {
                        width / aspect
                    } else {
                        ui.available_size_before_wrap().y
                    }
                })
                .at_least(min_size.y);
            vec2(width, height)
        };

        // Allocate the space.
        let (rect, response) = ui.allocate_exact_size(size, Sense::drag());

        // Load or initialize the memory.
        let plot_id = ui.make_persistent_id(id_source);
        let mut memory = PlotMemory::load(ui.ctx(), plot_id).unwrap_or_else(|| PlotMemory {
            auto_bounds: !min_auto_bounds.is_valid(),
            hovered_entry: None,
            hidden_items: Default::default(),
            min_auto_bounds,
            last_screen_transform: ScreenTransform::new(
                rect,
                min_auto_bounds,
                center_x_axis,
                center_y_axis,
            ),
            last_click_pos_for_zoom: None,
        });

        // If the min bounds changed, recalculate everything.
        if min_auto_bounds != memory.min_auto_bounds {
            memory = PlotMemory {
                auto_bounds: !min_auto_bounds.is_valid(),
                hovered_entry: None,
                min_auto_bounds,
                ..memory
            };
            memory.clone().store(ui.ctx(), plot_id);
        }

        let PlotMemory {
            mut auto_bounds,
            mut hovered_entry,
            mut hidden_items,
            last_screen_transform,
            mut last_click_pos_for_zoom,
            ..
        } = memory;

        // Call the plot build function.
        let mut plot_ui = PlotUi {
            items: Vec::new(),
            next_auto_color_idx: 0,
            last_screen_transform,
            response,
            ctx: ui.ctx().clone(),
        };
        let inner = build_fn(&mut plot_ui);
        let PlotUi {
            mut items,
            mut response,
            last_screen_transform,
            ..
        } = plot_ui;

        // Background
        if show_background {
            ui.painter().sub_region(rect).add(epaint::RectShape {
                rect,
                corner_radius: 2.0,
                fill: ui.visuals().extreme_bg_color,
                stroke: ui.visuals().widgets.noninteractive.bg_stroke,
            });
        }

        // --- Legend ---
        let legend = legend_config
            .and_then(|config| LegendWidget::try_new(rect, config, &items, &hidden_items));
        // Don't show hover cursor when hovering over legend.
        if hovered_entry.is_some() {
            hover_line = HoverLine::None;
        }
        // Remove the deselected items.
        items.retain(|item| !hidden_items.contains(item.name()));
        // Highlight the hovered items.
        if let Some(hovered_name) = &hovered_entry {
            items
                .iter_mut()
                .filter(|entry| entry.name() == hovered_name)
                .for_each(|entry| entry.highlight());
        }
        // Move highlighted items to front.
        items.sort_by_key(|item| item.highlighted());

        // --- Bound computation ---
        let mut bounds = *last_screen_transform.bounds();

        // Transfer the bounds from a link group.
        if let Some(axes) = linked_axes.as_ref() {
            if let Some(linked_bounds) = axes.get() {
                if axes.link_x {
                    bounds.min[0] = linked_bounds.min[0];
                    bounds.max[0] = linked_bounds.max[0];
                }
                if axes.link_y {
                    bounds.min[1] = linked_bounds.min[1];
                    bounds.max[1] = linked_bounds.max[1];
                }
                // Turn off auto bounds to keep it from overriding what we just set.
                auto_bounds = false;
            }
        }

        // Allow double clicking to reset to automatic bounds.
        auto_bounds |= response.double_clicked_by(PointerButton::Primary);

        // Set bounds automatically based on content.
        if auto_bounds || !bounds.is_valid() {
            bounds = min_auto_bounds;
            items
                .iter()
                .for_each(|item| bounds.merge(&item.get_bounds()));
            bounds.add_relative_margin(margin_fraction);
        }

        let mut transform = ScreenTransform::new(rect, bounds, center_x_axis, center_y_axis);

        // Enforce equal aspect ratio.
        if let Some(data_aspect) = data_aspect {
            let preserve_y = linked_axes
                .as_ref()
                .map_or(false, |group| group.link_y && !group.link_x);
            transform.set_aspect(data_aspect as f64, preserve_y);
        }

        // Dragging
        if allow_drag && response.dragged_by(PointerButton::Primary) {
            response = response.on_hover_cursor(CursorIcon::Grabbing);
            transform.translate_bounds(-response.drag_delta());
            auto_bounds = false;
        }

        // Zooming
        let mut boxed_zoom_rect = None;
        if allow_boxed_zoom {
            // Save last click to allow boxed zooming
            if response.drag_started() && response.dragged_by(boxed_zoom_pointer) {
                // it would be best for egui that input has a memory of the last click pos because it's a common pattern
                last_click_pos_for_zoom = response.hover_pos();
            }
            let box_start_pos = last_click_pos_for_zoom;
            let box_end_pos = response.hover_pos();
            if let (Some(box_start_pos), Some(box_end_pos)) = (box_start_pos, box_end_pos) {
                // while dragging prepare a Shape and draw it later on top of the plot
                if response.dragged_by(boxed_zoom_pointer) {
                    response = response.on_hover_cursor(CursorIcon::ZoomIn);
                    let rect = epaint::Rect::from_two_pos(box_start_pos, box_end_pos);
                    boxed_zoom_rect = Some((
                        epaint::RectShape::stroke(
                            rect,
                            0.0,
                            epaint::Stroke::new(4., Color32::DARK_BLUE),
                        ), // Outer stroke
                        epaint::RectShape::stroke(
                            rect,
                            0.0,
                            epaint::Stroke::new(2., Color32::WHITE),
                        ), // Inner stroke
                    ));
                }
                // when the click is release perform the zoom
                if response.drag_released() {
                    let box_start_pos = transform.value_from_position(box_start_pos);
                    let box_end_pos = transform.value_from_position(box_end_pos);
                    let new_bounds = PlotBounds {
                        min: [box_start_pos.x, box_end_pos.y],
                        max: [box_end_pos.x, box_start_pos.y],
                    };
                    if new_bounds.is_valid() {
                        *transform.bounds_mut() = new_bounds;
                        auto_bounds = false;
                    } else {
                        auto_bounds = true;
                    }
                    // reset the boxed zoom state
                    last_click_pos_for_zoom = None;
                }
            }
        }

        if allow_zoom {
            if let Some(hover_pos) = response.hover_pos() {
                let zoom_factor = if data_aspect.is_some() {
                    Vec2::splat(ui.input().zoom_delta())
                } else {
                    ui.input().zoom_delta_2d()
                };
                if zoom_factor != Vec2::splat(1.0) {
                    transform.zoom(zoom_factor, hover_pos);
                    auto_bounds = false;
                }

                let scroll_delta = ui.input().scroll_delta;
                if scroll_delta != Vec2::ZERO {
                    transform.translate_bounds(-scroll_delta);
                    auto_bounds = false;
                }
            }
        }

        // Initialize values from functions.
        items
            .iter_mut()
            .for_each(|item| item.initialize(transform.bounds().range_x()));

        let prepared = PreparedPlot {
            items,
            hover_line,
            show_hover_label,
            hover_formatter,
            axis_formatters,
            show_axes,
            transform: transform.clone(),
        };
        prepared.ui(ui, &response);

        if let Some(boxed_zoom_rect) = boxed_zoom_rect {
            ui.painter().sub_region(rect).add(boxed_zoom_rect.0);
            ui.painter().sub_region(rect).add(boxed_zoom_rect.1);
        }

        if let Some(mut legend) = legend {
            ui.add(&mut legend);
            hidden_items = legend.get_hidden_items();
            hovered_entry = legend.get_hovered_entry_name();
        }

        if let Some(group) = linked_axes.as_ref() {
            group.set(*transform.bounds());
        }

        let memory = PlotMemory {
            auto_bounds,
            hovered_entry,
            hidden_items,
            min_auto_bounds,
            last_screen_transform: transform,
            last_click_pos_for_zoom,
        };
        memory.store(ui.ctx(), plot_id);

        let response = if !matches!(hover_line, HoverLine::None) {
            response.on_hover_cursor(CursorIcon::Crosshair)
        } else {
            response
        };

        InnerResponse { inner, response }
    }
}

/// Provides methods to interact with a plot while building it. It is the single argument of the closure
/// provided to [`Plot::show`]. See [`Plot`] for an example of how to use it.
pub struct PlotUi {
    items: Vec<Box<dyn PlotItem>>,
    next_auto_color_idx: usize,
    last_screen_transform: ScreenTransform,
    response: Response,
    ctx: Context,
}

impl PlotUi {
    fn auto_color(&mut self) -> Color32 {
        let i = self.next_auto_color_idx;
        self.next_auto_color_idx += 1;
        let golden_ratio = (5.0_f32.sqrt() - 1.0) / 2.0; // 0.61803398875
        let h = i as f32 * golden_ratio;
        Hsva::new(h, 0.85, 0.5, 1.0).into() // TODO: OkLab or some other perspective color space
    }

    pub fn ctx(&self) -> &Context {
        &self.ctx
    }

    /// The plot bounds as they were in the last frame. If called on the first frame and the bounds were not
    /// further specified in the plot builder, this will return bounds centered on the origin. The bounds do
    /// not change until the plot is drawn.
    pub fn plot_bounds(&self) -> PlotBounds {
        *self.last_screen_transform.bounds()
    }

    /// Returns `true` if the plot area is currently hovered.
    pub fn plot_hovered(&self) -> bool {
        self.response.hovered()
    }

    /// The pointer position in plot coordinates. Independent of whether the pointer is in the plot area.
    pub fn pointer_coordinate(&self) -> Option<Value> {
        // We need to subtract the drag delta to keep in sync with the frame-delayed screen transform:
        let last_pos = self.ctx().input().pointer.latest_pos()? - self.response.drag_delta();
        let value = self.plot_from_screen(last_pos);
        Some(value)
    }

    /// The pointer drag delta in plot coordinates.
    pub fn pointer_coordinate_drag_delta(&self) -> Vec2 {
        let delta = self.response.drag_delta();
        let dp_dv = self.last_screen_transform.dpos_dvalue();
        Vec2::new(delta.x / dp_dv[0] as f32, delta.y / dp_dv[1] as f32)
    }

    /// Transform the plot coordinates to screen coordinates.
    pub fn screen_from_plot(&self, position: Value) -> Pos2 {
        self.last_screen_transform.position_from_value(&position)
    }

    /// Transform the screen coordinates to plot coordinates.
    pub fn plot_from_screen(&self, position: Pos2) -> Value {
        self.last_screen_transform.value_from_position(position)
    }

    /// Add a data line.
    pub fn line(&mut self, mut line: Line) {
        if line.series.is_empty() {
            return;
        };

        // Give the stroke an automatic color if no color has been assigned.
        if line.stroke.color == Color32::TRANSPARENT {
            line.stroke.color = self.auto_color();
        }
        self.items.push(Box::new(line));
    }

    /// Add a polygon. The polygon has to be convex.
    pub fn polygon(&mut self, mut polygon: Polygon) {
        if polygon.series.is_empty() {
            return;
        };

        // Give the stroke an automatic color if no color has been assigned.
        if polygon.stroke.color == Color32::TRANSPARENT {
            polygon.stroke.color = self.auto_color();
        }
        self.items.push(Box::new(polygon));
    }

    /// Add a text.
    pub fn text(&mut self, text: Text) {
        if text.text.is_empty() {
            return;
        };

        self.items.push(Box::new(text));
    }

    /// Add data points.
    pub fn points(&mut self, mut points: Points) {
        if points.series.is_empty() {
            return;
        };

        // Give the points an automatic color if no color has been assigned.
        if points.color == Color32::TRANSPARENT {
            points.color = self.auto_color();
        }
        self.items.push(Box::new(points));
    }

    /// Add arrows.
    pub fn arrows(&mut self, mut arrows: Arrows) {
        if arrows.origins.is_empty() || arrows.tips.is_empty() {
            return;
        };

        // Give the arrows an automatic color if no color has been assigned.
        if arrows.color == Color32::TRANSPARENT {
            arrows.color = self.auto_color();
        }
        self.items.push(Box::new(arrows));
    }

    /// Add an image.
    pub fn image(&mut self, image: PlotImage) {
        self.items.push(Box::new(image));
    }

    /// Add a horizontal line.
    /// Can be useful e.g. to show min/max bounds or similar.
    /// Always fills the full width of the plot.
    pub fn hline(&mut self, mut hline: HLine) {
        if hline.stroke.color == Color32::TRANSPARENT {
            hline.stroke.color = self.auto_color();
        }
        self.items.push(Box::new(hline));
    }

    /// Add a vertical line.
    /// Can be useful e.g. to show min/max bounds or similar.
    /// Always fills the full height of the plot.
    pub fn vline(&mut self, mut vline: VLine) {
        if vline.stroke.color == Color32::TRANSPARENT {
            vline.stroke.color = self.auto_color();
        }
        self.items.push(Box::new(vline));
    }

    /// Add a box plot diagram.
    pub fn box_plot(&mut self, mut box_plot: BoxPlot) {
        if box_plot.boxes.is_empty() {
            return;
        }

        // Give the elements an automatic color if no color has been assigned.
        if box_plot.default_color == Color32::TRANSPARENT {
            box_plot = box_plot.color(self.auto_color());
        }
        self.items.push(Box::new(box_plot));
    }

    /// Add a bar chart.
    pub fn bar_chart(&mut self, mut chart: BarChart) {
        if chart.bars.is_empty() {
            return;
        }

        // Give the elements an automatic color if no color has been assigned.
        if chart.default_color == Color32::TRANSPARENT {
            chart = chart.color(self.auto_color());
        }
        self.items.push(Box::new(chart));
    }
}

struct PreparedPlot {
    items: Vec<Box<dyn PlotItem>>,
    hover_line: HoverLine,
    show_hover_label: bool,
    hover_formatter: HoverFormatter,
    axis_formatters: [AxisFormatter; 2],
    show_axes: [bool; 2],
    transform: ScreenTransform,
}

impl PreparedPlot {
    fn ui(self, ui: &mut Ui, response: &Response) {
        let mut shapes = Vec::new();

        for d in 0..2 {
            if self.show_axes[d] {
                self.paint_axis(ui, d, &mut shapes);
            }
        }

        let transform = &self.transform;

        let mut plot_ui = ui.child_ui(*transform.frame(), Layout::default());
        plot_ui.set_clip_rect(*transform.frame());
        for item in &self.items {
            item.get_shapes(&mut plot_ui, transform, &mut shapes);
        }

        if let Some(pointer) = response.hover_pos() {
            self.hover(ui, pointer, &mut shapes);
        }

        ui.painter().sub_region(*transform.frame()).extend(shapes);
    }

    fn paint_axis(&self, ui: &Ui, axis: usize, shapes: &mut Vec<Shape>) {
        let Self {
            transform,
            axis_formatters,
            ..
        } = self;

        let bounds = transform.bounds();

        let font_id = TextStyle::Body.resolve(ui.style());

        let base: i64 = 10;
        let basef = base as f64;

        let min_line_spacing_in_points = 6.0; // TODO: large enough for a wide label
        let step_size = transform.dvalue_dpos()[axis] * min_line_spacing_in_points;
        let step_size = basef.powi(step_size.abs().log(basef).ceil() as i32);

        let step_size_in_points = (transform.dpos_dvalue()[axis] * step_size).abs() as f32;

        // Where on the cross-dimension to show the label values
        let value_cross = 0.0_f64.clamp(bounds.min[1 - axis], bounds.max[1 - axis]);

        for i in 0.. {
            let value_main = step_size * (bounds.min[axis] / step_size + i as f64).floor();
            if value_main > bounds.max[axis] {
                break;
            }

            let value = if axis == 0 {
                Value::new(value_main, value_cross)
            } else {
                Value::new(value_cross, value_main)
            };
            let pos_in_gui = transform.position_from_value(&value);

            let n = (value_main / step_size).round() as i64;
            let spacing_in_points = if n % (base * base) == 0 {
                step_size_in_points * (basef * basef) as f32 // think line (multiple of 100)
            } else if n % base == 0 {
                step_size_in_points * basef as f32 // medium line (multiple of 10)
            } else {
                step_size_in_points // thin line
            };

            let line_alpha = remap_clamp(
                spacing_in_points,
                (min_line_spacing_in_points as f32)..=300.0,
                0.0..=0.15,
            );

            if line_alpha > 0.0 {
                let line_color = color_from_alpha(ui, line_alpha);

                let mut p0 = pos_in_gui;
                let mut p1 = pos_in_gui;
                p0[1 - axis] = transform.frame().min[1 - axis];
                p1[1 - axis] = transform.frame().max[1 - axis];
                shapes.push(Shape::line_segment([p0, p1], Stroke::new(1.0, line_color)));
            }

            let text_alpha = remap_clamp(spacing_in_points, 40.0..=150.0, 0.0..=0.4);

            if text_alpha > 0.0 {
                let color = color_from_alpha(ui, text_alpha);

                let text: String = if let Some(formatter) = axis_formatters[axis].as_deref() {
                    formatter(value_main)
                } else {
                    emath::round_to_decimals(value_main, 5).to_string() // hack
                };

                // Custom formatters can return empty string to signal "no label at this resolution"
                if !text.is_empty() {
                    let galley = ui.painter().layout_no_wrap(text, font_id.clone(), color);

                    let mut text_pos = pos_in_gui + vec2(1.0, -galley.size().y);

                    // Make sure we see the labels, even if the axis is off-screen:
                    text_pos[1 - axis] = text_pos[1 - axis]
                        .at_most(transform.frame().max[1 - axis] - galley.size()[1 - axis] - 2.0)
                        .at_least(transform.frame().min[1 - axis] + 1.0);

                    shapes.push(Shape::galley(text_pos, galley));
                }
            }
        }

        fn color_from_alpha(ui: &Ui, alpha: f32) -> Color32 {
            if ui.visuals().dark_mode {
                Rgba::from_white_alpha(alpha).into()
            } else {
                Rgba::from_black_alpha((4.0 * alpha).at_most(1.0)).into()
            }
        }
    }

    fn hover(&self, ui: &Ui, pointer: Pos2, shapes: &mut Vec<Shape>) {
        let Self {
            transform,
            hover_line,
            show_hover_label,
            hover_formatter,
            items,
            ..
        } = self;

        if matches!(hover_line, HoverLine::None) && !show_hover_label {
            return;
        }

        let interact_radius_sq: f32 = (16.0f32).powi(2);

        let candidates = items.iter().filter_map(|item| {
            let item = &**item;
            let closest = item.find_closest(pointer, transform);

            Some(item).zip(closest)
        });

        let closest = candidates
            .min_by_key(|(_, elem)| elem.dist_sq.ord())
            .filter(|(_, elem)| elem.dist_sq <= interact_radius_sq);

        let plot = items::PlotConfig {
            ui,
            transform,
            hover_config: HoverConfig {
                hover_line: *hover_line,
                show_hover_label: *show_hover_label,
            },
            hover_formatter,
        };

        if let Some((item, elem)) = closest {
            item.on_hover(elem, shapes, &plot);
        } else {
            let value = transform.value_from_position(pointer);
            items::rulers_at_value(pointer, value, "", &plot, shapes);
        }
    }
}
