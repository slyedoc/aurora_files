#![allow(deprecated)]

use super::{
    clothing::ClothingMenu,
    dropdown::{DropdownMenu, FilterOptions, matches_filter},
};
use bevy::{
    ecs::bundle::Bundle,
    feathers::{controls::*, theme::*, tokens},
    picking::hover::Hovered,
    prelude::*,
    ui_widgets::*,
};
use crate::{assets::*, components::*, prelude::Morph};

use super::{
    scroll::*,
    text_input::{self, TextInputProps},
};

#[derive(Component)]
pub struct MorphsSection;

#[derive(Component)]
struct MorphsList;

#[derive(Component)]
pub struct MorphMenu;

#[derive(Component)]
struct MorphOption(MorphTarget);

#[derive(Component)]
pub struct MorphFilterInput;

#[derive(Component)]
struct MorphOptionsContainer;

#[derive(Component)]
struct MorphItem(usize);

#[derive(EntityEvent)]
struct MorphSelect {
    entity: Entity,
    target: MorphTarget,
}

#[derive(EntityEvent)]
struct MorphRemove {
    entity: Entity,
    idx: usize,
}

#[derive(EntityEvent)]
struct MorphValueChange {
    entity: Entity,
    idx: usize,
    value: f32,
}

#[derive(EntityEvent)]
struct MorphClose {
    entity: Entity,
}

pub fn morphs_section(human_entity: Entity, morphs: &Morphs) -> impl Bundle {
    let items: Vec<_> = morphs
        .iter()
        .enumerate()
        .map(|(idx, morph)| morph_item_row(human_entity, idx, morph))
        .collect();
    (
        Name::new("MorphsSection"),
        MorphsSection,
        Node {
            flex_direction: FlexDirection::Column,
            padding: UiRect::top(Val::Px(8.0)),
            ..default()
        },
        children![
            (
                Text::new("Morphs"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                ThemedText
            ),
            (
                Name::new("MorphsList"),
                MorphsList,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    padding: UiRect::bottom(Val::Px(4.0)),
                    ..default()
                },
                Children::spawn(SpawnIter(items.into_iter()))
            ),
            (
                Name::new("AddMorphButton"),
                button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new("+ Add"), ThemedText))
                ),
                observe(on_open_morph_menu(human_entity))
            ),
        ],
        observe(on_morph_select(human_entity)),
        observe(on_morph_remove(human_entity)),
        observe(on_morph_value_change(human_entity)),
        observe(on_morph_close),
        observe(on_morph_filter),
    )
}

fn morph_item_row(_human_entity: Entity, idx: usize, morph: &Morph) -> impl Bundle {
    let (min, max) = morph.target.value_range();
    let label = format!("{:?}", morph.target);
    (
        Name::new(format!("MorphItem_{}", idx)),
        MorphItem(idx),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(4.0),
            width: Val::Percent(100.0),
            ..default()
        },
        children![
            (
                Text::new(label),
                TextFont {
                    font_size: FontSize::Px(10.0),
                    ..default()
                },
                ThemedText,
                Node {
                    width: Val::Px(120.0),
                    overflow: Overflow::clip(),
                    ..default()
                }
            ),
            (
                Node {
                    flex_grow: 1.0,
                    ..default()
                },
                children![(
                    slider_bundle(
                        FeathersSliderProps {
                            value: morph.value,
                            min,
                            max
                        },
                        (SliderStep(0.01), SliderPrecision(2))
                    ),
                    observe(on_morph_slider_change)
                )]
            ),
            (
                Node {
                    width: Val::Px(24.0),
                    height: Val::Px(24.0),
                    ..default()
                },
                children![(
                    button_bundle(
                        ButtonBundleProps::default(),
                        (),
                        Spawn((Text::new("×"), ThemedText))
                    ),
                    observe(on_morph_item_remove_click)
                )]
            ),
        ],
    )
}

fn on_morph_slider_change(
    trigger: On<ValueChange<f32>>,
    mut commands: Commands,
    parent_query: Query<&ChildOf>,
    item_query: Query<&MorphItem>,
    section_query: Query<&MorphsSection>,
) {
    commands
        .entity(trigger.source)
        .insert(SliderValue(trigger.value));
    for ancestor in parent_query.iter_ancestors(trigger.source) {
        if let Ok(item) = item_query.get(ancestor) {
            for section_ancestor in parent_query.iter_ancestors(ancestor) {
                if section_query.get(section_ancestor).is_ok() {
                    commands.trigger(MorphValueChange {
                        entity: section_ancestor,
                        idx: item.0,
                        value: trigger.value,
                    });
                    return;
                }
            }
        }
    }
}

fn on_morph_value_change(
    human_entity: Entity,
) -> impl FnMut(On<MorphValueChange>, Commands, Query<&mut Morphs>) {
    move |trigger: On<MorphValueChange>,
          mut commands: Commands,
          mut morphs_query: Query<&mut Morphs>| {
        if let Ok(mut morphs) = morphs_query.get_mut(human_entity) {
            if trigger.idx < morphs.len() {
                morphs[trigger.idx].value = trigger.value;
                commands.entity(human_entity).insert(HumanDirty);
            }
        }
    }
}

fn on_morph_item_remove_click(
    trigger: On<Pointer<Click>>,
    mut commands: Commands,
    parent_query: Query<&ChildOf>,
    item_query: Query<&MorphItem>,
    section_query: Query<&MorphsSection>,
) {
    for ancestor in parent_query.iter_ancestors(trigger.entity) {
        if let Ok(item) = item_query.get(ancestor) {
            for section_ancestor in parent_query.iter_ancestors(ancestor) {
                if section_query.get(section_ancestor).is_ok() {
                    commands.trigger(MorphRemove {
                        entity: section_ancestor,
                        idx: item.0,
                    });
                    return;
                }
            }
        }
    }
}

fn on_morph_remove(
    human_entity: Entity,
) -> impl FnMut(
    On<MorphRemove>,
    Commands,
    Query<&mut Morphs>,
    Query<&Children>,
    Query<Entity, With<MorphsList>>,
) {
    move |trigger: On<MorphRemove>,
          mut commands: Commands,
          mut morphs_query: Query<&mut Morphs>,
          children_query: Query<&Children>,
          list_query: Query<Entity, With<MorphsList>>| {
        if let Ok(mut morphs) = morphs_query.get_mut(human_entity) {
            if trigger.idx < morphs.len() {
                morphs.remove(trigger.idx);
                commands.entity(human_entity).insert(HumanDirty);
                for child in children_query.iter_descendants(trigger.entity) {
                    if let Ok(list_entity) = list_query.get(child) {
                        commands.entity(list_entity).despawn_children();
                        for (idx, morph) in morphs.iter().enumerate() {
                            commands.entity(list_entity).with_child(morph_item_row(
                                human_entity,
                                idx,
                                morph,
                            ));
                        }
                        break;
                    }
                }
            }
        }
    }
}

fn on_morph_select(
    human_entity: Entity,
) -> impl FnMut(
    On<MorphSelect>,
    Commands,
    Query<&mut Morphs>,
    Query<&Children>,
    Query<Entity, With<MorphsList>>,
) {
    move |trigger: On<MorphSelect>,
          mut commands: Commands,
          mut morphs_query: Query<&mut Morphs>,
          children_query: Query<&Children>,
          list_query: Query<Entity, With<MorphsList>>| {
        commands.trigger(MorphClose {
            entity: trigger.entity,
        });
        if let Ok(mut morphs) = morphs_query.get_mut(human_entity) {
            if !morphs.iter().any(|m| m.target == trigger.target) {
                let new_morph = Morph::new(trigger.target, 0.0);
                morphs.push(new_morph.clone());
                commands.entity(human_entity).insert(HumanDirty);
                for child in children_query.iter_descendants(trigger.entity) {
                    if let Ok(list_entity) = list_query.get(child) {
                        let idx = morphs.len() - 1;
                        commands.entity(list_entity).with_child(morph_item_row(
                            human_entity,
                            idx,
                            &new_morph,
                        ));
                        break;
                    }
                }
            }
        }
    }
}

fn on_morph_close(
    trigger: On<MorphClose>,
    mut commands: Commands,
    children_query: Query<&Children>,
    menu_query: Query<&MorphMenu>,
) {
    for child in children_query.iter_descendants(trigger.entity) {
        if menu_query.get(child).is_ok() {
            commands.entity(child).despawn();
        }
    }
}

fn on_close_morph_menu_click(
    trigger: On<Pointer<Click>>,
    mut commands: Commands,
    parent_query: Query<&ChildOf>,
    section_query: Query<&MorphsSection>,
) {
    if let Some(section) = parent_query
        .iter_ancestors(trigger.entity)
        .find(|e| section_query.get(*e).is_ok())
    {
        commands.trigger(MorphClose { entity: section });
    }
}

fn on_morph_filter(
    trigger: On<FilterOptions>,
    children_query: Query<&Children>,
    options_container: Query<Entity, With<MorphOptionsContainer>>,
    mut query: Query<(&MorphOption, &mut Node)>,
) {
    for child in children_query.iter_descendants(trigger.entity) {
        if let Ok(container) = options_container.get(child) {
            for c in children_query.iter_descendants(container) {
                if let Ok((option, mut node)) = query.get_mut(c) {
                    let label = format!("{:?}", option.0);
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

fn on_open_morph_menu(
    human_entity: Entity,
) -> impl FnMut(
    On<Pointer<Click>>,
    Commands,
    Query<&Children>,
    Query<&MorphMenu>,
    Query<&ChildOf>,
    Query<&MorphsSection>,
    Query<Entity, Or<(With<DropdownMenu>, With<ClothingMenu>, With<MorphMenu>)>>,
) {
    move |trigger: On<Pointer<Click>>,
          mut commands: Commands,
          children_query: Query<&Children>,
          menu_query: Query<&MorphMenu>,
          parent_query: Query<&ChildOf>,
          section_query: Query<&MorphsSection>,
          all_menus: Query<
        Entity,
        Or<(With<DropdownMenu>, With<ClothingMenu>, With<MorphMenu>)>,
    >| {
        let section_entity = parent_query
            .iter_ancestors(trigger.entity)
            .find(|e| section_query.get(*e).is_ok())
            .unwrap_or(trigger.entity);
        for child in children_query.iter_descendants(section_entity) {
            if menu_query.get(child).is_ok() {
                return;
            }
        }
        for menu in all_menus.iter() {
            commands.entity(menu).despawn();
        }

        let options: Vec<_> = MorphTarget::iter().collect();
        let option_bundles: Vec<_> = options
            .into_iter()
            .map(|target| {
                let label = format!("{:?}", target);
                (
                    Name::new(label.clone()),
                    MorphOption(target),
                    Node {
                        width: Val::Percent(100.0),
                        align_items: AlignItems::Center,
                        min_height: Val::Px(24.0),
                        ..default()
                    },
                    children![(
                        button_bundle(
                            ButtonBundleProps::default(),
                            (),
                            Spawn((
                                Text::new(label),
                                ThemedText,
                                TextFont {
                                    font_size: FontSize::Px(10.0),
                                    ..default()
                                }
                            ))
                        ),
                        observe(on_morph_option_click(human_entity, target))
                    )],
                )
            })
            .collect();

        commands.entity(section_entity).with_child((
            MorphMenu,
            Name::new("MorphMenu"),
            Node {
                flex_direction: FlexDirection::Column,
                width: Val::Percent(100.0),
                ..default()
            },
            ThemeBackgroundColor(tokens::WINDOW_BG),
            ZIndex(10),
            children![
                (
                    Node {
                        flex_direction: FlexDirection::Row,
                        width: Val::Percent(100.0),
                        ..default()
                    },
                    children![
                        (
                            MorphFilterInput,
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
                            observe(on_close_morph_menu_click)
                        ),
                    ]
                ),
                scroll(
                    ScrollProps::vertical(px(300.0)),
                    (MorphOptionsContainer, Name::new("MorphOptions")),
                    Children::spawn(SpawnIter(option_bundles.into_iter()))
                ),
            ],
            Hovered(false),
        ));
    }
}

fn on_morph_option_click(
    _human_entity: Entity,
    target: MorphTarget,
) -> impl FnMut(On<Pointer<Click>>, Commands, Query<&ChildOf>, Query<&MorphsSection>) {
    move |trigger: On<Pointer<Click>>,
          mut commands: Commands,
          parent_query: Query<&ChildOf>,
          section_query: Query<&MorphsSection>| {
        if let Some(section) = parent_query
            .iter_ancestors(trigger.entity)
            .find(|e| section_query.get(*e).is_ok())
        {
            commands.trigger(MorphSelect {
                entity: section,
                target,
            });
        }
    }
}
