#![allow(deprecated)]

use super::{
    clothing::ClothingMenu,
    morphs::MorphMenu,
    scroll::*,
    text_input::{self, TextInputProps},
};

use bevy::{
    feathers::{controls::*, theme::*, tokens},
    picking::hover::Hovered,
    prelude::*,
    ui_widgets::*,
};
use crate::assets::*;
use bevy::text::EditableText;
use strum::IntoEnumIterator;

pub fn filter_text_changed<T: Component, K: Component>(
    filter_query: Query<(Entity, &EditableText), (With<T>, Changed<EditableText>)>,
    parent_query: Query<&ChildOf>,
    section_query: Query<&K>,
    mut commands: Commands,
) {
    for (e, text) in filter_query.iter() {
        for c in parent_query.iter_ancestors(e) {
            if section_query.contains(c) {
                commands.trigger(FilterOptions {
                    entity: c,
                    filter: text.value().to_string(),
                });
            }
        }
    }
}

#[derive(Component)]
pub struct Dropdown;

#[derive(Component)]
struct DropdownButton;

#[derive(Component)]
pub struct DropdownMenu;

#[derive(Component)]
pub struct DropdownFilterInput;

#[derive(Component)]
struct DropdownOptionsContainer;

#[derive(EntityEvent)]
struct DropdownSelect<T: Component + Copy + Send + Sync + 'static> {
    entity: Entity,
    value: T,
}

#[derive(EntityEvent)]
struct DropdownSelectOptional<T: Component + Copy + Send + Sync + 'static> {
    entity: Entity,
    value: Option<T>,
}

#[derive(EntityEvent)]
struct DropdownClose {
    entity: Entity,
}

#[derive(EntityEvent)]
pub struct FilterOptions {
    pub entity: Entity,
    pub filter: String,
}

pub fn dropdown<T: Component + Copy + IntoEnumIterator + ToString + Send + Sync + 'static>(
    human_entity: Entity,
    value: T,
) -> impl Bundle {
    let type_name = std::any::type_name::<T>()
        .split("::")
        .last()
        .unwrap_or_default();
    (
        Name::new(format!("Dropdown{}", type_name)),
        Dropdown,
        Node {
            flex_direction: FlexDirection::Column,
            ..default()
        },
        children![
            (
                Text::new(type_name),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                ThemedText
            ),
            (
                Name::new("DropdownButton"),
                DropdownButton,
                button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new(value.to_string()), ThemedText))
                ),
                observe(on_open_dropdown::<T>),
            ),
        ],
        observe(on_dropdown_select::<T>(human_entity)),
        observe(on_dropdown_close),
        observe(on_dropdown_filter::<T>),
    )
}

pub fn dropdown_with_thumb<
    T: Component + Copy + IntoEnumIterator + ToString + MHThumb + Send + Sync + 'static,
>(
    human_entity: Entity,
    value: T,
) -> impl Bundle {
    let type_name = std::any::type_name::<T>()
        .split("::")
        .last()
        .unwrap_or_default();
    (
        Dropdown,
        Name::new(format!("Dropdown {}", type_name)),
        Node {
            padding: UiRect::top(Val::Px(8.0)),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        children![
            (
                Text::new(type_name),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                ThemedText
            ),
            (
                Name::new("DropdownButton"),
                DropdownButton,
                button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new(value.to_string()), ThemedText))
                ),
                observe(on_open_dropdown_thumb::<T>),
            ),
        ],
        observe(on_dropdown_select::<T>(human_entity)),
        observe(on_dropdown_close),
        observe(on_dropdown_filter::<T>),
    )
}

pub fn dropdown_optional_with_thumb<
    T: Component + Copy + IntoEnumIterator + ToString + MHThumb + Send + Sync + 'static,
>(
    human_entity: Entity,
    value: Option<&T>,
) -> impl Bundle {
    let type_name = std::any::type_name::<T>()
        .split("::")
        .last()
        .unwrap_or_default();
    let label = value.map_or("None".to_string(), |v| v.to_string());
    (
        Dropdown,
        Name::new(format!("DropdownOptional {}", type_name)),
        Node {
            padding: UiRect::top(Val::Px(8.0)),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        children![
            (
                Text::new(type_name),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                ThemedText
            ),
            (
                Name::new("DropdownButton"),
                DropdownButton,
                button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new(label), ThemedText))
                ),
                observe(on_open_dropdown_optional_thumb::<T>),
            ),
        ],
        observe(on_dropdown_select_optional::<T>(human_entity)),
        observe(on_dropdown_close),
        observe(on_dropdown_filter_optional::<T>),
    )
}

fn on_dropdown_select_optional<
    T: Component + Copy + IntoEnumIterator + ToString + Send + Sync + 'static,
>(
    human_entity: Entity,
) -> impl FnMut(On<DropdownSelectOptional<T>>, Commands, Query<&DropdownButton>, Query<&Children>) {
    move |trigger: On<DropdownSelectOptional<T>>,
          mut commands: Commands,
          dropdown_button_query: Query<&DropdownButton>,
          children_query: Query<&Children>| {
        commands.trigger(DropdownClose {
            entity: trigger.entity,
        });

        match trigger.value {
            Some(value) => {
                commands.entity(human_entity).insert(value);
            }
            None => {
                commands.entity(human_entity).remove::<T>();
            }
        }

        // Update button label
        let label = trigger.value.map_or("None".to_string(), |v| v.to_string());
        for e in children_query.iter_descendants(trigger.entity) {
            if dropdown_button_query.get(e).is_ok() {
                commands
                    .entity(e)
                    .despawn_children()
                    .with_child((Text::new(label), ThemedText));
                break;
            }
        }
    }
}

fn on_option_click_optional<
    T: Component + Copy + IntoEnumIterator + ToString + Send + Sync + 'static,
>(
    value: Option<T>,
) -> impl FnMut(On<Pointer<Click>>, Commands, Query<&ChildOf>, Query<&Dropdown>) {
    move |trigger: On<Pointer<Click>>,
          mut commands: Commands,
          parent_query: Query<&ChildOf>,
          dropdown_query: Query<&Dropdown>| {
        let parent = parent_query
            .iter_ancestors(trigger.entity)
            .find(|c| dropdown_query.get(*c).is_ok())
            .unwrap();
        commands.trigger(DropdownSelectOptional {
            entity: parent,
            value,
        });
    }
}

/// Marker for "None" option row (doesn't have T component)
#[derive(Component)]
struct DropdownNoneOption;

fn on_dropdown_filter_optional<
    T: Component + Copy + IntoEnumIterator + ToString + Send + Sync + 'static,
>(
    trigger: On<FilterOptions>,
    children_query: Query<&Children>,
    dropdown_options_container: Query<Entity, With<DropdownOptionsContainer>>,
    mut query: Query<(&T, &mut Node)>,
    mut none_query: Query<&mut Node, (With<DropdownNoneOption>, Without<T>)>,
) {
    for child in children_query.iter_descendants(trigger.entity) {
        if let Ok(container) = dropdown_options_container.get(child) {
            for c in children_query.iter_descendants(container) {
                // Handle T options
                if let Ok((value, mut node)) = query.get_mut(c) {
                    let label = value.to_string();
                    node.display = if matches_filter(&label, &trigger.filter) {
                        Display::Flex
                    } else {
                        Display::None
                    };
                }
                // Handle None option
                if let Ok(mut node) = none_query.get_mut(c) {
                    node.display = if matches_filter("none", &trigger.filter) {
                        Display::Flex
                    } else {
                        Display::None
                    };
                }
            }
            break;
        }
    }
}

fn on_open_dropdown_optional_thumb<
    T: Component + Copy + IntoEnumIterator + ToString + MHThumb + Send + Sync + 'static,
>(
    trigger: On<Pointer<Click>>,
    children_query: Query<&Children>,
    dropdown_open: Query<&DropdownMenu>,
    parent_query: Query<&ChildOf>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    all_menus: Query<Entity, Or<(With<DropdownMenu>, With<ClothingMenu>, With<MorphMenu>)>>,
) {
    let child_of = parent_query.get(trigger.entity).unwrap();
    for child in children_query.iter_descendants(child_of.0) {
        if dropdown_open.get(child).is_ok() {
            return;
        }
    }
    for menu in all_menus.iter() {
        commands.entity(menu).despawn();
    }

    let options: Vec<_> = T::iter()
        .map(|value| {
            let label = value.to_string();
            let image = asset_server.load::<Image>(value.thumb());
            (
                Name::new(label.clone()),
                Node {
                    width: Val::Percent(100.0),
                    align_items: AlignItems::Start,
                    min_height: Val::Px(28.0),
                    ..default()
                },
                value,
                children![
                    (
                        Node {
                            width: Val::Px(24.0),
                            height: Val::Px(24.0),
                            ..default()
                        },
                        ImageNode { image, ..default() }
                    ),
                    (
                        button_bundle(
                            ButtonBundleProps::default(),
                            (),
                            Spawn((Text::new(label), ThemedText))
                        ),
                        observe(on_option_click_optional(Some(value)))
                    ),
                ],
            )
        })
        .collect();

    commands.entity(child_of.0).with_child((
        DropdownMenu,
        Name::new("DropdownOpen"),
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            ..default()
        },
        ThemeBackgroundColor(tokens::WINDOW_BG),
        children![
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    width: Val::Percent(100.0),
                    ..default()
                },
                children![
                    (
                        DropdownFilterInput,
                        text_input::text_input(
                            TextInputProps {
                                width: Val::Percent(100.0),
                                height: Val::Px(24.0),
                                // (placeholder text: not supported by EditableText yet)
                                ..default()
                            },
                            ()
                        )
                    ),
                    (
                        button_bundle(
                            ButtonBundleProps::default(),
                            (),
                            Spawn((Text::new("×"), ThemedText))
                        ),
                        observe(on_close_dropdown_click)
                    ),
                ]
            ),
            // None option (outside scroll, always visible at top)
            (
                Name::new("None"),
                DropdownNoneOption,
                Node {
                    width: Val::Percent(100.0),
                    align_items: AlignItems::Start,
                    min_height: Val::Px(28.0),
                    padding: UiRect::horizontal(Val::Px(4.0)),
                    ..default()
                },
                children![(
                    button_bundle(
                        ButtonBundleProps::default(),
                        (),
                        Spawn((Text::new("None"), ThemedText))
                    ),
                    observe(on_option_click_optional::<T>(None))
                )],
            ),
            // Scrollable value options
            scroll(
                ScrollProps::vertical(px(400.0)),
                (DropdownOptionsContainer, Name::new("OptionsContainer")),
                Children::spawn(SpawnIter(options.into_iter()))
            ),
        ],
        Hovered(false),
    ));
}

fn on_dropdown_select<T: Component + Copy + IntoEnumIterator + ToString + Send + Sync + 'static>(
    human_entity: Entity,
) -> impl FnMut(On<DropdownSelect<T>>, Commands, Query<&DropdownButton>, Query<&Children>) {
    move |trigger: On<DropdownSelect<T>>,
          mut commands: Commands,
          dropdown_button_query: Query<&DropdownButton>,
          children_query: Query<&Children>| {
        commands.trigger(DropdownClose {
            entity: trigger.entity,
        });
        commands.entity(human_entity).insert(trigger.value);

        // Update button label
        for e in children_query.iter_descendants(trigger.entity) {
            if dropdown_button_query.get(e).is_ok() {
                commands
                    .entity(e)
                    .despawn_children()
                    .with_child((Text::new(trigger.value.to_string()), ThemedText));
                break;
            }
        }
    }
}

fn on_option_click<T: Component + Copy + IntoEnumIterator + ToString + Send + Sync + 'static>(
    value: T,
) -> impl FnMut(On<Pointer<Click>>, Commands, Query<&ChildOf>, Query<&Dropdown>) {
    move |trigger: On<Pointer<Click>>,
          mut commands: Commands,
          parent_query: Query<&ChildOf>,
          dropdown_query: Query<&Dropdown>| {
        let parent = parent_query
            .iter_ancestors(trigger.entity)
            .find(|c| dropdown_query.get(*c).is_ok())
            .unwrap();
        commands.trigger(DropdownSelect {
            entity: parent,
            value,
        });
    }
}

fn on_dropdown_close(
    trigger: On<DropdownClose>,
    mut commands: Commands,
    children_query: Query<&Children>,
    dropdown_open: Query<&DropdownMenu>,
) {
    for child in children_query.get(trigger.entity).unwrap().iter() {
        if dropdown_open.get(child).is_ok() {
            commands.entity(child).despawn();
        }
    }
}

fn on_close_dropdown_click(
    trigger: On<Pointer<Click>>,
    mut commands: Commands,
    parent_query: Query<&ChildOf>,
    dropdown_query: Query<&Dropdown>,
) {
    if let Some(dropdown) = parent_query
        .iter_ancestors(trigger.entity)
        .find(|e| dropdown_query.get(*e).is_ok())
    {
        commands.trigger(DropdownClose { entity: dropdown });
    }
}

/// Check if label matches all filter terms (space-separated)
/// Prefix term with `-` to exclude (e.g. "old -female" matches "old" but not "female")
pub fn matches_filter(label: &str, filter: &str) -> bool {
    if filter.is_empty() {
        return true;
    }
    let label = label.to_lowercase();
    filter.to_lowercase().split_whitespace().all(|term| {
        if let Some(excluded) = term.strip_prefix('-') {
            !excluded.is_empty() && !label.contains(excluded)
        } else {
            label.contains(term)
        }
    })
}

fn on_dropdown_filter<T: Component + Copy + IntoEnumIterator + ToString + Send + Sync + 'static>(
    trigger: On<FilterOptions>,
    children_query: Query<&Children>,
    dropdown_options_container: Query<Entity, With<DropdownOptionsContainer>>,
    mut query: Query<(&T, &mut Node)>,
) {
    for child in children_query.iter_descendants(trigger.entity) {
        if let Ok(container) = dropdown_options_container.get(child) {
            for c in children_query.iter_descendants(container) {
                if let Ok((value, mut node)) = query.get_mut(c) {
                    let label = value.to_string();
                    node.display = if matches_filter(&label, &trigger.filter) {
                        Display::Flex
                    } else {
                        Display::None
                    };
                }
            }
            break;
        }
    }
}

fn on_open_dropdown<T: Component + Copy + IntoEnumIterator + ToString + Send + Sync + 'static>(
    trigger: On<Pointer<Click>>,
    children_query: Query<&Children>,
    dropdown_open: Query<&DropdownMenu>,
    parent_query: Query<&ChildOf>,
    mut commands: Commands,
    all_menus: Query<Entity, Or<(With<DropdownMenu>, With<ClothingMenu>, With<MorphMenu>)>>,
) {
    let child_of = parent_query.get(trigger.entity).unwrap();
    for child in children_query.iter_descendants(child_of.0) {
        if dropdown_open.get(child).is_ok() {
            return;
        }
    }
    for menu in all_menus.iter() {
        commands.entity(menu).despawn();
    }

    let options: Vec<_> = T::iter()
        .map(|value| {
            let label = value.to_string();
            (
                Name::new(label.clone()),
                Node {
                    width: Val::Percent(100.0),
                    min_height: Val::Px(28.0),
                    ..default()
                },
                value,
                children![(
                    button_bundle(
                        ButtonBundleProps::default(),
                        (),
                        Spawn((Text::new(label), ThemedText))
                    ),
                    observe(on_option_click(value))
                )],
            )
        })
        .collect();

    commands.entity(child_of.0).with_child((
        DropdownMenu,
        Name::new("DropdownOpen"),
        Node {
            padding: UiRect::top(Val::Px(4.0)),
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            ..default()
        },
        ThemeBackgroundColor(tokens::BUTTON_BG),
        children![
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    width: Val::Percent(100.0),
                    ..default()
                },
                children![
                    (
                        DropdownFilterInput,
                        text_input::text_input(
                            TextInputProps {
                                width: Val::Percent(100.0),
                                height: Val::Px(24.0),
                                // (placeholder text: not supported by EditableText yet)
                                ..default()
                            },
                            ()
                        )
                    ),
                    (
                        button_bundle(
                            ButtonBundleProps::default(),
                            (),
                            Spawn((Text::new("×"), ThemedText))
                        ),
                        observe(on_close_dropdown_click)
                    ),
                ]
            ),
            scroll(
                ScrollProps::vertical(px(400.0)),
                (DropdownOptionsContainer, Name::new("OptionsContainer")),
                Children::spawn(SpawnIter(options.into_iter()))
            ),
        ],
        Hovered(false),
    ));
}

fn on_open_dropdown_thumb<
    T: Component + Copy + IntoEnumIterator + ToString + MHThumb + Send + Sync + 'static,
>(
    trigger: On<Pointer<Click>>,
    children_query: Query<&Children>,
    dropdown_open: Query<&DropdownMenu>,
    parent_query: Query<&ChildOf>,
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    all_menus: Query<Entity, Or<(With<DropdownMenu>, With<ClothingMenu>, With<MorphMenu>)>>,
) {
    let child_of = parent_query.get(trigger.entity).unwrap();
    for child in children_query.iter_descendants(child_of.0) {
        if dropdown_open.get(child).is_ok() {
            return;
        }
    }
    for menu in all_menus.iter() {
        commands.entity(menu).despawn();
    }

    let options: Vec<_> = T::iter()
        .map(|value| {
            let label = value.to_string();
            let image = asset_server.load::<Image>(value.thumb());
            (
                Name::new(label.clone()),
                Node {
                    width: Val::Percent(100.0),
                    align_items: AlignItems::Start,
                    min_height: Val::Px(28.0),
                    ..default()
                },
                value,
                children![
                    (
                        Node {
                            width: Val::Px(24.0),
                            height: Val::Px(24.0),
                            ..default()
                        },
                        ImageNode { image, ..default() }
                    ),
                    (
                        button_bundle(
                            ButtonBundleProps::default(),
                            (),
                            Spawn((Text::new(label), ThemedText))
                        ),
                        observe(on_option_click(value))
                    ),
                ],
            )
        })
        .collect();

    commands.entity(child_of.0).with_child((
        DropdownMenu,
        Name::new("DropdownOpen"),
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            ..default()
        },
        ThemeBackgroundColor(tokens::WINDOW_BG),
        children![
            (
                Node {
                    flex_direction: FlexDirection::Row,
                    width: Val::Percent(100.0),
                    ..default()
                },
                children![
                    (
                        DropdownFilterInput,
                        text_input::text_input(
                            TextInputProps {
                                width: Val::Percent(100.0),
                                height: Val::Px(24.0),
                                // (placeholder text: not supported by EditableText yet)
                                ..default()
                            },
                            ()
                        )
                    ),
                    (
                        button_bundle(
                            ButtonBundleProps::default(),
                            (),
                            Spawn((Text::new("×"), ThemedText))
                        ),
                        observe(on_close_dropdown_click)
                    ),
                ]
            ),
            scroll(
                ScrollProps::vertical(px(400.0)),
                (DropdownOptionsContainer, Name::new("OptionsContainer")),
                Children::spawn(SpawnIter(options.into_iter()))
            ),
        ],
        Hovered(false),
    ));
}
