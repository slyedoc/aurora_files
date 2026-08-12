#![allow(deprecated)]

use bevy::{
    ecs::bundle::Bundle,
    feathers::{controls::*, theme::*},
    prelude::*,
    ui_widgets::*,
};

#[derive(Component)]
pub struct Collapsible;

#[derive(Component)]
pub struct CollapsibleContent;

pub fn collapsible<C: Bundle>(title: &'static str, expanded: bool, content: C) -> impl Bundle {
    (
        Name::new(format!("Collapsible{}", title)),
        Collapsible,
        Node {
            flex_direction: FlexDirection::Column,
            width: Val::Percent(100.0),
            padding: UiRect::top(Val::Px(4.0)),
            ..default()
        },
        children![
            (
                Name::new("CollapsibleHeader"),
                button_bundle(
                    ButtonBundleProps::default(),
                    (),
                    Spawn((
                        Node {
                            width: Val::Percent(100.0),
                            ..default()
                        },
                        Text::new(format!("{} {}", if expanded { "v" } else { ">" }, title)),
                        ThemedText,
                        TextFont {
                            font_size: FontSize::Px(12.0),
                            ..default()
                        },
                    )),
                ),
                observe(on_collapsible_toggle),
            ),
            (
                Name::new("CollapsibleContent"),
                CollapsibleContent,
                Node {
                    flex_direction: FlexDirection::Column,
                    width: Val::Percent(100.0),
                    padding: UiRect::left(Val::Px(8.0)),
                    display: if expanded {
                        Display::Flex
                    } else {
                        Display::None
                    },
                    ..default()
                },
                content,
            ),
        ],
    )
}

pub fn collapse_all(
    _trigger: On<Pointer<Click>>,
    mut content_query: Query<&mut Node, With<CollapsibleContent>>,
    mut text_query: Query<&mut Text>,
    collapsible_query: Query<&Children, With<Collapsible>>,
    children_query: Query<&Children>,
) {
    for mut node in content_query.iter_mut() {
        node.display = Display::None;
    }
    for children in collapsible_query.iter() {
        for child in children.iter() {
            for desc in children_query.iter_descendants(child) {
                if let Ok(mut text) = text_query.get_mut(desc) {
                    if text.0.starts_with("v") {
                        text.0 = text.0.replacen("v", ">", 1);
                    }
                    break;
                }
            }
            break;
        }
    }
}

pub fn expand_all(
    _trigger: On<Pointer<Click>>,
    mut content_query: Query<&mut Node, With<CollapsibleContent>>,
    mut text_query: Query<&mut Text>,
    collapsible_query: Query<&Children, With<Collapsible>>,
    children_query: Query<&Children>,
) {
    for mut node in content_query.iter_mut() {
        node.display = Display::Flex;
    }
    for children in collapsible_query.iter() {
        for child in children.iter() {
            for desc in children_query.iter_descendants(child) {
                if let Ok(mut text) = text_query.get_mut(desc) {
                    if text.0.starts_with(">") {
                        text.0 = text.0.replacen(">", "v", 1);
                    }
                    break;
                }
            }
            break;
        }
    }
}

pub fn on_collapsible_toggle(
    trigger: On<Pointer<Click>>,
    parent_query: Query<&ChildOf>,
    collapsible_query: Query<&Collapsible>,
    children_query: Query<&Children>,
    mut content_query: Query<&mut Node, With<CollapsibleContent>>,
    mut text_query: Query<&mut Text>,
) {
    let Some(collapsible) = parent_query
        .iter_ancestors(trigger.entity)
        .find(|e| collapsible_query.get(*e).is_ok())
    else {
        return;
    };

    for child in children_query.iter_descendants(collapsible) {
        if let Ok(mut node) = content_query.get_mut(child) {
            let is_expanded = node.display == Display::Flex;
            node.display = if is_expanded {
                Display::None
            } else {
                Display::Flex
            };

            for desc in children_query.iter_descendants(trigger.entity) {
                if let Ok(mut text) = text_query.get_mut(desc) {
                    let current = text.0.clone();
                    if current.starts_with("v") {
                        text.0 = current.replacen("v", ">", 1);
                    } else if current.starts_with(">") {
                        text.0 = current.replacen(">", "v", 1);
                    }
                    break;
                }
            }
            break;
        }
    }
}
