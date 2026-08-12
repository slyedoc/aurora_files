use bevy::ecs::bundle::Bundle;
use bevy::feathers::{
    constants::size,
    theme::{InheritableThemeTextColor, ThemeBackgroundColor, ThemeBorderColor},
    tokens,
};
use bevy::input_focus::{InputFocus, tab_navigation::TabIndex};
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::text::{EditableText, TextCursorStyle};
use bevy::ui::{AlignItems, Display, FlexDirection, Node, UiRect, Val};

use bevy_enhanced_input::prelude::*;

pub struct TextInputProps {
    pub width: Val,
    pub height: Val,
    pub initial_text: String,
    pub max_chars: Option<usize>,
}

impl Default for TextInputProps {
    fn default() -> Self {
        Self {
            width: Val::Px(280.0),
            height: size::ROW_HEIGHT,
            initial_text: String::new(),
            max_chars: None,
        }
    }
}

/// Single-line editable text field on bevy's built-in `EditableText`
pub fn text_input<B: Bundle>(props: TextInputProps, overrides: B) -> impl Bundle {
    let mut editable = EditableText::new(&props.initial_text);
    editable.max_characters = props.max_chars;

    (
        Node {
            width: props.width,
            height: props.height,
            display: Display::Flex,
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..Default::default()
        },
        editable,
        TextLayout { linebreak: LineBreak::NoWrap, ..Default::default() },
        TextCursorStyle::default(),
        Hovered::default(),
        TabIndex(0),
        ThemeBackgroundColor(tokens::BUTTON_BG),
        ThemeBorderColor(tokens::CHECKBOX_BORDER),
        InheritableThemeTextColor(tokens::BUTTON_TEXT),
        overrides,
    )
}

/// System to enable/disable a bevy_enhanced_input context based on text input focus.
///
/// When a text input has focus, the context is disabled.
/// When focus is lost or moves to a non-text-input element, the context is enabled.
///
/// This is useful for preventing camera controls or other input actions from triggering
/// while typing in text fields.
///
/// # Usage
///
/// ```rust,ignore
/// app.add_systems(Update,
///     handle_text_input_focus::<CameraFree>.run_if(resource_changed::<InputFocus>)
/// );
/// ```
pub fn handle_text_input_focus<T>(
    input_focus: Res<InputFocus>,
    text_input_query: Query<&EditableText>,
    context_query: Query<Entity, With<T>>,
    mut commands: Commands,
) where
    T: Component,
{
    let Ok(context_entity) = context_query.single() else {
        return;
    };

    if let Some(focused_entity) = input_focus.get() {
        // Check if the focused entity is a text input
        if text_input_query.contains(focused_entity) {
            // Disable context when text input has focus
            commands
                .entity(context_entity)
                .insert(ContextActivity::<T>::INACTIVE);
        } else {
            // Enable context if focused entity is not a text input
            commands
                .entity(context_entity)
                .insert(ContextActivity::<T>::ACTIVE);
        }
    } else {
        // Enable context when nothing has focus
        commands
            .entity(context_entity)
            .insert(ContextActivity::<T>::ACTIVE);
    }
}
