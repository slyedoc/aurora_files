use bevy::{
    ecs::{bundle::Bundle, observer::On, system::Query},
    feathers::{theme::ThemeBackgroundColor, tokens},
    picking::events::{Pointer, Scroll as ScrollEvent},
    ui::{
        AlignItems, ComputedNode, JustifyContent, Node, Overflow, OverflowAxis, PositionType,
        ScrollPosition, UiRect, Val,
    },
    ui_widgets::observe,
};

/// Scrollbar styling constants
const LINE_HEIGHT: f32 = 21.0;

/// Parameters for the scroll container template.
pub struct ScrollProps {
    /// Width of the scroll container
    pub width: Val,
    /// Height of the scroll container
    pub height: Val,
    /// Overflow settings (horizontal, vertical, or both)
    pub overflow: Overflow,
    /// Flex direction for content layout
    pub flex_direction: bevy::ui::FlexDirection,
    /// Align items (horizontal alignment for column layouts, vertical for row layouts)
    pub align_items: AlignItems,
}

impl Default for ScrollProps {
    fn default() -> Self {
        Self {
            width: Val::Percent(100.0),
            height: Val::Auto,
            overflow: Overflow::hidden(),
            flex_direction: bevy::ui::FlexDirection::Column,
            align_items: AlignItems::Stretch,
        }
    }
}

impl ScrollProps {
    /// Create a vertically scrolling container with default styling
    pub fn vertical(height: Val) -> Self {
        Self {
            width: Val::Percent(100.0),
            height,
            overflow: Overflow {
                x: OverflowAxis::Hidden,
                y: OverflowAxis::Scroll,
            },
            flex_direction: bevy::ui::FlexDirection::Column,
            align_items: AlignItems::Stretch,
        }
    }

    /// Create a horizontally scrolling container with default styling
    #[allow(dead_code)]
    pub fn horizontal(width: Val) -> Self {
        Self {
            width,
            height: Val::Auto,
            overflow: Overflow {
                x: OverflowAxis::Scroll,
                y: OverflowAxis::Hidden,
            },
            flex_direction: bevy::ui::FlexDirection::Row,
            align_items: AlignItems::Stretch,
        }
    }

    /// Create a bidirectionally scrolling container with default styling
    #[allow(dead_code)]
    pub fn both(width: Val, height: Val) -> Self {
        Self {
            width,
            height,
            overflow: Overflow {
                x: OverflowAxis::Scroll,
                y: OverflowAxis::Scroll,
            },
            flex_direction: bevy::ui::FlexDirection::Column,
            align_items: AlignItems::Stretch,
        }
    }
}

/// Simple scroll container without scrollbar thumbs.
/// TODO: Add scrollbar thumbs back when bevy 0.18.0 is released (CoreScrollbarThumb fix)
/// NOTE: BorderRadius removed due to 0.18.0-rc.2 bug (missing Component derive)
pub fn scroll<C: Bundle, B: Bundle>(props: ScrollProps, overrides: B, children: C) -> impl Bundle {
    let padding = UiRect::all(Val::Px(4.0));

    (
        Node {
            width: props.width,
            height: props.height,
            flex_direction: props.flex_direction,
            justify_content: JustifyContent::Start,
            align_items: props.align_items,
            overflow: props.overflow,
            padding,
            position_type: PositionType::Relative,
            ..Default::default()
        },
        ScrollPosition::default(),
        ThemeBackgroundColor(tokens::WINDOW_BG),
        observe(scroll_observer),
        children,
        overrides,
    )
}

/// Observer that handles scroll events for UI containers with ScrollPosition.
fn scroll_observer(
    scroll: On<Pointer<ScrollEvent>>,
    mut query: Query<(&mut ScrollPosition, &Node, &ComputedNode)>,
) {
    let Ok((mut scroll_position, node, computed)) = query.get_mut(scroll.entity) else {
        return;
    };

    let event = scroll.event();
    let mut delta = -bevy::math::Vec2::new(event.x, event.y);

    // Convert line units to pixels
    if delta.x.abs() < 10.0 && delta.y.abs() < 10.0 {
        delta *= LINE_HEIGHT;
    }

    // If only horizontal scrolling is enabled and we have vertical scroll input,
    // convert the vertical scroll to horizontal
    if node.overflow.x == OverflowAxis::Scroll
        && node.overflow.y != OverflowAxis::Scroll
        && delta.x == 0.
        && delta.y != 0.
    {
        delta.x = delta.y;
        delta.y = 0.;
    }

    let max_offset = (computed.content_size() - computed.size()) * computed.inverse_scale_factor();

    // Handle horizontal scrolling
    if node.overflow.x == OverflowAxis::Scroll && delta.x != 0. {
        let at_limit = if delta.x > 0. {
            scroll_position.x >= max_offset.x
        } else {
            scroll_position.x <= 0.
        };

        if !at_limit {
            scroll_position.x = (scroll_position.x + delta.x).clamp(0., max_offset.x);
        }
    }

    // Handle vertical scrolling
    if node.overflow.y == OverflowAxis::Scroll && delta.y != 0. {
        let at_limit = if delta.y > 0. {
            scroll_position.y >= max_offset.y
        } else {
            scroll_position.y <= 0.
        };

        if !at_limit {
            scroll_position.y = (scroll_position.y + delta.y).clamp(0., max_offset.y);
        }
    }
}
