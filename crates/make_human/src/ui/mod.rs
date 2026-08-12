#![allow(deprecated)]

pub mod clothing;
pub mod collapsible;
pub mod dropdown;
pub mod morphs;
pub mod offset;
pub mod scroll;
pub mod text_input;

use bevy::{
    feathers::{
        controls::{ButtonBundleProps, button_bundle},
        theme::ThemedText,
    },
    prelude::*,
    ui_widgets::observe,
};
use crate::prelude::*;

use clothing::clothing_section;
use collapsible::collapsible;
use dropdown::{dropdown, dropdown_optional_with_thumb, dropdown_with_thumb};
use morphs::morphs_section;
use offset::offset_slider;
use scroll::{ScrollProps, scroll};

/// Plugin to provide feathers editor widget
pub struct MakeHumanUiPlugin;

impl Plugin for MakeHumanUiPlugin {
    fn build(&self, app: &mut bevy::app::App) {
        app.add_systems(
            Update,
            (
                dropdown::filter_text_changed::<dropdown::DropdownFilterInput, dropdown::Dropdown>,
                dropdown::filter_text_changed::<
                    clothing::ClothingFilterInput,
                    clothing::ClothingSection,
                >,
                dropdown::filter_text_changed::<morphs::MorphFilterInput, morphs::MorphsSection>,
            ),
        )
        .add_observer(on_human_editor_add);
    }
}

/// Marker component for human editor widget. Stores which human entity to edit.
/// Content is built via observer on add.
#[derive(Component)]
pub struct HumanEditor(pub Entity);

fn on_human_editor_add(
    trigger: On<Add, HumanEditor>,
    mut commands: Commands,
    editor_query: Query<&HumanEditor>,
    human_query: Query<HumanQuery>,
) {
    let entity = trigger.entity;
    let Ok(editor) = editor_query.get(entity) else {
        return;
    };
    let human_entity = editor.0;

    let Ok(h) = human_query.get(human_entity) else {
        return;
    };

    // Extract values
    let name = h
        .name
        .map_or_else(|| "Unnamed".to_string(), |n| n.to_string());
    let rig = *h.rig;
    let skin_mesh = *h.skin_mesh;
    let skin_material = *h.skin_material;
    let floor_offset = h.floor_offset.0;
    let eyes = *h.eyes;
    let eyebrows = *h.eyebrows;
    let eyelashes = *h.eyelashes;
    let teeth = *h.teeth;
    let tongue = *h.tongue;
    let clothing = h.clothing.clone();
    let clothing_offset = h.clothing_offset.0;
    let morphs = h.morphs.clone();

    commands.entity(entity).with_child(scroll(
        ScrollProps::vertical(percent(100.)),
        (),
        children![
            // Header
            (
                Name::new("HeaderRow"),
                Node {
                    flex_direction: FlexDirection::Row,
                    width: Val::Percent(100.0),
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    ..default()
                },
                children![
                    (Text::new(name), ThemedText, TextFont::from_font_size(14.0),),
                    (
                        Node {
                            flex_direction: FlexDirection::Row,
                            column_gap: Val::Px(4.0),
                            ..default()
                        },
                        children![
                            (
                                button_bundle(
                                    ButtonBundleProps::default(),
                                    (),
                                    Spawn((Text::new("-"), ThemedText))
                                ),
                                observe(collapsible::collapse_all)
                            ),
                            (
                                button_bundle(
                                    ButtonBundleProps::default(),
                                    (),
                                    Spawn((Text::new("+"), ThemedText))
                                ),
                                observe(collapsible::expand_all)
                            ),
                        ],
                    ),
                ],
            ),
            // Sections
            collapsible(
                "General",
                true,
                children![
                    dropdown::<Rig>(human_entity, rig),
                    dropdown_with_thumb::<SkinMesh>(human_entity, skin_mesh),
                    dropdown_with_thumb::<SkinMaterial>(human_entity, skin_material),
                    offset_slider::<FloorOffset>(human_entity, "Floor", floor_offset, -0.1, 0.1),
                ]
            ),
            collapsible(
                "Head",
                true,
                children![
                    dropdown_optional_with_thumb::<Hair>(human_entity, h.hair),
                    dropdown_with_thumb::<Eyes>(human_entity, eyes),
                    dropdown_with_thumb::<Eyebrows>(human_entity, eyebrows),
                    dropdown_with_thumb::<Eyelashes>(human_entity, eyelashes),
                    dropdown_with_thumb::<Teeth>(human_entity, teeth),
                    dropdown_with_thumb::<Tongue>(human_entity, tongue),
                ]
            ),
            collapsible(
                "Clothes",
                true,
                children![
                    clothing_section(human_entity, &clothing),
                    offset_slider::<ClothingOffset>(
                        human_entity,
                        "Offset",
                        clothing_offset,
                        0.0,
                        0.01
                    ),
                ]
            ),
            collapsible(
                "Morphs",
                false,
                children![morphs_section(human_entity, &morphs),]
            ),
        ],
    ));
}

/// Create a human editor widget for the given entity.
///
/// # Example
/// ```ignore
/// commands.spawn(human_editor(human_entity, ()));
/// ```
pub fn human_editor<B: Bundle>(human: Entity, overrides: B) -> impl Bundle {
    (
        Name::new("HumanEditor"),
        HumanEditor(human),
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        overrides,
    )
}
