#![allow(deprecated)]

use bevy::{
    feathers::{
        controls::{ButtonBundleProps, button_bundle},
        theme::{ThemeBackgroundColor, ThemedText},
        tokens,
    },
    picking::hover::Hovered,
    prelude::*,
    ui_widgets::observe,
};
use strum::IntoEnumIterator;

use crate::{
    assets::*,
    components::*,
    ui::{
        dropdown::{DropdownMenu, FilterOptions, matches_filter},
        morphs::MorphMenu,
        scroll::{ScrollProps, scroll},
        text_input::{TextInputProps, text_input},
    },
};

#[derive(Component)]
pub struct ClothingSection;

#[derive(Component)]
struct ClothingList;

#[derive(Component)]
pub struct ClothingMenu;

#[derive(Component)]
struct ClothingOption(Clothing);

#[derive(Component)]
pub struct ClothingFilterInput;

#[derive(Component)]
struct ClothingOptionsContainer;

#[derive(Component)]
struct ClothingItem(usize);

#[derive(EntityEvent)]
struct ClothingSelect {
    entity: Entity,
    item: Clothing,
}

#[derive(EntityEvent)]
struct ClothingRemove {
    entity: Entity,
    idx: usize,
}

#[derive(EntityEvent)]
struct ClothingClose {
    entity: Entity,
}

pub fn clothing_section(human_entity: Entity, outfit: &Outfit) -> impl Bundle {
    let items: Vec<_> = outfit
        .iter()
        .enumerate()
        .map(|(idx, item)| clothing_item_row(human_entity, idx, *item))
        .collect();
    (
        Name::new("ClothingSection"),
        ClothingSection,
        Node {
            flex_direction: FlexDirection::Column,
            padding: UiRect::top(Val::Px(8.0)),
            ..default()
        },
        children![
            (
                Text::new("Clothing"),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                ThemedText
            ),
            (
                Name::new("ClothingList"),
                ClothingList,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(4.0),
                    padding: UiRect::bottom(Val::Px(4.0)),
                    ..default()
                },
                Children::spawn(SpawnIter(items.into_iter()))
            ),
            (
                Name::new("AddClothingButton"),
                button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((Text::new("+ Add"), ThemedText))
                ),
                observe(on_open_clothing_menu(human_entity))
            ),
        ],
        observe(on_clothing_select(human_entity)),
        observe(on_clothing_remove(human_entity)),
        observe(on_clothing_close),
        observe(on_clothing_filter),
    )
}

fn clothing_item_row(_human_entity: Entity, idx: usize, item: Clothing) -> impl Bundle {
    (
        Name::new(format!("ClothingItem_{}", idx)),
        ClothingItem(idx),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: Val::Px(4.0),
            overflow: Overflow::clip(),
            ..default()
        },
        children![
            (
                Text::new(item.to_string()),
                TextFont {
                    font_size: FontSize::Px(12.0),
                    ..default()
                },
                ThemedText,
                Node {
                    flex_grow: 1.0,
                    ..default()
                }
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
                    observe(on_clothing_item_remove_click)
                )]
            ),
        ],
    )
}

fn on_clothing_item_remove_click(
    trigger: On<Pointer<Click>>,
    mut commands: Commands,
    parent_query: Query<&ChildOf>,
    item_query: Query<&ClothingItem>,
    section_query: Query<&ClothingSection>,
) {
    for ancestor in parent_query.iter_ancestors(trigger.entity) {
        if let Ok(item) = item_query.get(ancestor) {
            for section_ancestor in parent_query.iter_ancestors(ancestor) {
                if section_query.get(section_ancestor).is_ok() {
                    commands.trigger(ClothingRemove {
                        entity: section_ancestor,
                        idx: item.0,
                    });
                    return;
                }
            }
        }
    }
}

fn on_clothing_remove(
    human_entity: Entity,
) -> impl FnMut(
    On<ClothingRemove>,
    Commands,
    Query<&mut Outfit>,
    Query<&Children>,
    Query<Entity, With<ClothingList>>,
) {
    move |trigger: On<ClothingRemove>,
          mut commands: Commands,
          mut outfit_query: Query<&mut Outfit>,
          children_query: Query<&Children>,
          list_query: Query<Entity, With<ClothingList>>| {
        if let Ok(mut outfit) = outfit_query.get_mut(human_entity) {
            if trigger.idx < outfit.len() {
                outfit.remove(trigger.idx);
                commands.entity(human_entity).insert(HumanDirty);
                for child in children_query.iter_descendants(trigger.entity) {
                    if let Ok(list_entity) = list_query.get(child) {
                        commands.entity(list_entity).despawn_children();
                        for (idx, item) in outfit.iter().enumerate() {
                            commands.entity(list_entity).with_child(clothing_item_row(
                                human_entity,
                                idx,
                                *item,
                            ));
                        }
                        break;
                    }
                }
            }
        }
    }
}

fn on_clothing_select(
    human_entity: Entity,
) -> impl FnMut(
    On<ClothingSelect>,
    Commands,
    Query<&mut Outfit>,
    Query<&Children>,
    Query<Entity, With<ClothingList>>,
) {
    move |trigger: On<ClothingSelect>,
          mut commands: Commands,
          mut outfit_query: Query<&mut Outfit>,
          children_query: Query<&Children>,
          list_query: Query<Entity, With<ClothingList>>| {
        commands.trigger(ClothingClose {
            entity: trigger.entity,
        });
        if let Ok(mut outfit) = outfit_query.get_mut(human_entity) {
            if !outfit.contains(&trigger.item) {
                outfit.push(trigger.item);
                commands.entity(human_entity).insert(HumanDirty);
                for child in children_query.iter_descendants(trigger.entity) {
                    if let Ok(list_entity) = list_query.get(child) {
                        let idx = outfit.len() - 1;
                        commands.entity(list_entity).with_child(clothing_item_row(
                            human_entity,
                            idx,
                            trigger.item,
                        ));
                        break;
                    }
                }
            }
        }
    }
}

fn on_clothing_close(
    trigger: On<ClothingClose>,
    mut commands: Commands,
    children_query: Query<&Children>,
    menu_query: Query<&ClothingMenu>,
) {
    for child in children_query.iter_descendants(trigger.entity) {
        if menu_query.get(child).is_ok() {
            commands.entity(child).despawn();
        }
    }
}

fn on_close_clothing_menu_click(
    trigger: On<Pointer<Click>>,
    mut commands: Commands,
    parent_query: Query<&ChildOf>,
    section_query: Query<&ClothingSection>,
) {
    if let Some(section) = parent_query
        .iter_ancestors(trigger.entity)
        .find(|e| section_query.get(*e).is_ok())
    {
        commands.trigger(ClothingClose { entity: section });
    }
}

fn on_clothing_filter(
    trigger: On<FilterOptions>,
    children_query: Query<&Children>,
    options_container: Query<Entity, With<ClothingOptionsContainer>>,
    mut query: Query<(&ClothingOption, &mut Node)>,
) {
    for child in children_query.iter_descendants(trigger.entity) {
        if let Ok(container) = options_container.get(child) {
            for c in children_query.iter_descendants(container) {
                if let Ok((option, mut node)) = query.get_mut(c) {
                    let label = option.0.to_string();
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

fn on_open_clothing_menu(
    human_entity: Entity,
) -> impl FnMut(
    On<Pointer<Click>>,
    Commands,
    Res<AssetServer>,
    Query<&Children>,
    Query<&ClothingMenu>,
    Query<&ChildOf>,
    Query<&ClothingSection>,
    Query<Entity, Or<(With<DropdownMenu>, With<ClothingMenu>, With<MorphMenu>)>>,
) {
    move |trigger: On<Pointer<Click>>,
          mut commands: Commands,
          asset_server: Res<AssetServer>,
          children_query: Query<&Children>,
          menu_query: Query<&ClothingMenu>,
          parent_query: Query<&ChildOf>,
          section_query: Query<&ClothingSection>,
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

        let options: Vec<_> = Clothing::iter()
            .map(|item| {
                let label = item.to_string();
                let image = asset_server.load::<Image>(item.thumb());
                (
                    Name::new(label.clone()),
                    ClothingOption(item),
                    Node {
                        width: Val::Percent(100.0),
                        align_items: AlignItems::Center,
                        min_height: Val::Px(28.0),
                        ..default()
                    },
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
                            observe(on_clothing_option_click(human_entity, item))
                        ),
                    ],
                )
            })
            .collect();

        commands.entity(section_entity).with_child((
            ClothingMenu,
            Name::new("ClothingMenu"),
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
                            ClothingFilterInput,
                            text_input(
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
                            observe(on_close_clothing_menu_click)
                        ),
                    ]
                ),
                scroll(
                    ScrollProps::vertical(px(300.0)),
                    (ClothingOptionsContainer, Name::new("ClothingOptions")),
                    Children::spawn(SpawnIter(options.into_iter()))
                ),
            ],
            Hovered(false),
        ));
    }
}

fn on_clothing_option_click(
    _human_entity: Entity,
    item: Clothing,
) -> impl FnMut(On<Pointer<Click>>, Commands, Query<&ChildOf>, Query<&ClothingSection>) {
    move |trigger: On<Pointer<Click>>,
          mut commands: Commands,
          parent_query: Query<&ChildOf>,
          section_query: Query<&ClothingSection>| {
        if let Some(section) = parent_query
            .iter_ancestors(trigger.entity)
            .find(|e| section_query.get(*e).is_ok())
        {
            commands.trigger(ClothingSelect {
                entity: section,
                item,
            });
        }
    }
}
