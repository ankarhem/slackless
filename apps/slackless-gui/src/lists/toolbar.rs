//! Live Slack List toolbar, search bar, inline add-item bar, CSV import/export, and sharing menu.

use super::*;

pub(super) fn render_live_add_bar(
    data: &SlackListData,
    list_key: &str,
    primary_ix: usize,
    state: &ListPanelState,
    actions: &ListActions,
    cx: &App,
) -> Option<Div> {
    let theme = cx.theme();
    let list_key = list_key.to_owned();
    let add_bar = state.adding.then(|| {
        state.new_item_input.clone().and_then(|input| {
            let create_cb = actions.on_create_item.clone()?;
            let primary_col_id = data.columns.get(primary_ix)?.id.clone();
            let input_submit = input.clone();
            let key_save = list_key.clone();
            let key_cancel = list_key.clone();
            Some(
                h_flex()
                    .items_center()
                    .gap_2()
                    .p_2()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.secondary.opacity(0.3))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(Textarea::new(&input).appearance(false).bordered(false)),
                    )
                    .child(
                        Button::new("list-live-add-save")
                            .primary()
                            .with_size(Size::XSmall)
                            .label("Add")
                            .on_click(move |_, window, cx| {
                                let text = input_submit.read(cx).value().trim().to_owned();
                                if !text.is_empty() {
                                    mutate_panel(
                                        &key_save,
                                        |s| {
                                            s.adding = false;
                                            s.new_item_input = None;
                                        },
                                        window,
                                        cx,
                                    );
                                    create_cb(
                                        vec![ListCellInput {
                                            column_id: primary_col_id.clone(),
                                            value: ListCellInputValue::Text(text),
                                        }],
                                        window,
                                        cx,
                                    );
                                }
                            }),
                    )
                    .child(
                        Button::new("list-live-add-cancel")
                            .ghost()
                            .with_size(Size::XSmall)
                            .icon(IconName::X)
                            .on_click(move |_, window, cx| {
                                mutate_panel(
                                    &key_cancel,
                                    |s| {
                                        s.adding = false;
                                        s.new_item_input = None;
                                    },
                                    window,
                                    cx,
                                );
                            }),
                    ),
            )
        })
    });
    add_bar.flatten()
}

pub(super) fn render_live_find_bar(
    query: &str,
    search: Option<&Entity<TextareaState>>,
    cx: &App,
) -> Option<Div> {
    let theme = cx.theme();
    search.map(|input| {
        h_flex()
            .w(px(280.))
            .h(px(30.))
            .px_2p5()
            .gap_2()
            .items_center()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .child(
                Icon::new(IconName::Search)
                    .size(px(14.))
                    .text_color(theme.muted_foreground),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .child(Textarea::new(input).appearance(false).bordered(false)),
            )
            .when(!query.trim().is_empty(), |row| {
                let clear_input = input.clone();
                row.child(
                    Button::new("list-find-clear")
                        .ghost()
                        .with_size(Size::XSmall)
                        .icon(IconName::X)
                        .on_click(move |_, window, cx| {
                            clear_input.update(cx, |state, cx| {
                                state.set_value(String::new(), window, cx);
                            });
                        }),
                )
            })
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn render_live_toolbar(
    data: &SlackListData,
    fixtures: &Fixtures,
    list_key: &str,
    primary_ix: usize,
    visible_count: usize,
    archived_count: usize,
    completed_to_archive: &[String],
    toggleable_cols: Vec<(String, String)>,
    state: &ListPanelState,
    find_bar: Option<Div>,
    actions: &ListActions,
    cx: &App,
) -> Div {
    let theme = cx.theme();
    let list_key = list_key.to_owned();
    let can_edit = actions.on_update_cell.is_some() || actions.on_create_item.is_some();
    h_flex()
        .items_center()
        .flex_wrap()
        .gap_2()
        .child(
            Icon::new(IconName::ListChecks)
                .text_color(theme.success)
                .size(px(18.)),
        )
        .child(
            v_flex()
                .min_w_0()
                .child(
                    h_flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(data.title.clone()),
                        )
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(format!("{} of {} items", visible_count, data.rows.len())),
                        ),
                )
                .when(!data.description.is_empty(), |col| {
                    col.child(
                        div()
                            .text_xs()
                            .text_color(theme.muted_foreground)
                            .child(data.description.clone()),
                    )
                }),
        )
        .child(div().flex_1())
        .when(
            actions.on_create_item.is_some() && !data.columns.is_empty(),
            |row| {
                let key_add = list_key.clone();
                row.child(
                    Button::new("list-live-add-item")
                        .primary()
                        .with_size(Size::XSmall)
                        .icon(IconName::Plus)
                        .label("Add item")
                        .on_click(move |_, window, cx| {
                            let input = cx.new(|cx| {
                                TextareaState::new(window, cx).placeholder("New item title…")
                            });
                            mutate_panel(
                                &key_add,
                                |s| {
                                    s.adding = true;
                                    s.new_item_input = Some(input);
                                },
                                window,
                                cx,
                            );
                        }),
                )
            },
        )
        .children(find_bar)
        .child(layout_switcher(&list_key, state.layout, cx))
        .child({
            let key_show = list_key.clone();
            let showing = state.show_archived;
            let label = if showing {
                "Hide archived".to_owned()
            } else if archived_count > 0 {
                format!("Show archived ({archived_count})")
            } else {
                "Show archived".to_owned()
            };
            Button::new("list-toggle-archived")
                .when(showing, |b| b.primary())
                .when(!showing, |b| b.outline())
                .with_size(Size::XSmall)
                .icon(IconName::Archive)
                .label(label)
                .tooltip("Toggle showing archived items in this view")
                .on_click(move |_, window, cx| {
                    mutate_panel(
                        &key_show,
                        |s| s.show_archived = !s.show_archived,
                        window,
                        cx,
                    );
                })
        })
        .when(!completed_to_archive.is_empty(), |row| {
            let key_bulk = list_key.clone();
            let ids = completed_to_archive.to_vec();
            let on_arch = actions.on_archive_item.clone();
            let count = ids.len();
            row.child(
                Button::new("list-archive-completed")
                    .outline()
                    .with_size(Size::XSmall)
                    .icon(IconName::Archive)
                    .label(format!("Archive completed ({count})"))
                    .tooltip("Archive all completed items in this view")
                    .on_click(move |_, window, cx| {
                        let ids_local = ids.clone();
                        mutate_panel(
                            &key_bulk,
                            |s| {
                                for id in &ids_local {
                                    s.archived_rows.insert(id.clone());
                                }
                                s.selected_row = None;
                            },
                            window,
                            cx,
                        );
                        if let Some(cb) = &on_arch {
                            for id in &ids {
                                cb(id.clone(), window, cx);
                            }
                        }
                    }),
            )
        })
        .when(!toggleable_cols.is_empty(), |row| {
            row.child(fields_menu(&list_key, toggleable_cols, &state.hidden_cols))
        })
        .when_some(actions.on_create_item.clone(), |row, create_cb| {
            let cols_meta: Vec<(String, String, String)> = data
                .columns
                .iter()
                .map(|c| (c.id.clone(), c.name.clone(), c.kind.clone()))
                .collect();
            let primary_col_ix = primary_ix;
            row.child(
                Button::new("list-import-csv")
                    .outline()
                    .with_size(Size::XSmall)
                    .icon(IconName::FileText)
                    .label("Import CSV")
                    .tooltip("Import rows from a local CSV file")
                    .on_click(move |_, window, cx| {
                        let cb = create_cb.clone();
                        let cols = cols_meta.clone();
                        let rx = cx.prompt_for_paths(gpui_kit::gpui::PathPromptOptions {
                            files: true,
                            directories: false,
                            multiple: false,
                            prompt: Some("Choose CSV file to import".into()),
                        });
                        window
                            .spawn(cx, async move |cx| {
                                let Ok(Ok(Some(paths))) = rx.await else {
                                    return;
                                };
                                let Some(path) = paths.into_iter().next() else {
                                    return;
                                };
                                let Ok(content) = std::fs::read_to_string(&path) else {
                                    return;
                                };
                                let rows = parse_csv_rows(&content);
                                if rows.len() < 2 || cols.is_empty() {
                                    return;
                                }
                                let headers = &rows[0];
                                let mut imported = 0usize;
                                let _ = cx.update(|window, cx| {
                                    for row_vals in rows.iter().skip(1).take(100) {
                                        let cells = map_csv_row_to_cells(
                                            headers,
                                            row_vals,
                                            &cols,
                                            primary_col_ix,
                                        );
                                        if !cells.is_empty() {
                                            cb(cells, window, cx);
                                            imported += 1;
                                        }
                                    }
                                    crate::actions::notify(
                                        "CSV imported",
                                        format!(
                                            "Imported {imported} row{} into list.",
                                            if imported == 1 { "" } else { "s" }
                                        ),
                                        window,
                                        cx,
                                    );
                                });
                            })
                            .detach();
                    }),
            )
        })
        .child({
            let csv = list_to_csv(data, fixtures);
            let suggested = format!("{}.csv", sanitize_filename(&data.title));
            Button::new("list-export-csv")
                .outline()
                .with_size(Size::XSmall)
                .icon(IconName::Download)
                .label("CSV")
                .tooltip("Download list as CSV")
                .on_click(move |_, window, cx| {
                    export_file(csv.clone(), suggested.clone(), "CSV", window, cx);
                })
        })
        .child({
            let json = list_to_json(data, fixtures);
            let suggested = format!("{}.json", sanitize_filename(&data.title));
            Button::new("list-export-json")
                .outline()
                .with_size(Size::XSmall)
                .icon(IconName::Download)
                .label("JSON")
                .tooltip("Download list as JSON")
                .on_click(move |_, window, cx| {
                    export_file(json.clone(), suggested.clone(), "JSON", window, cx);
                })
        })
        .when(!can_edit, |row| row.child(read_only_badge(cx)))
        .when_some(actions.on_set_access.clone(), |row, set_access| {
            row.child(
                Button::new("list-share-access")
                    .outline()
                    .with_size(Size::XSmall)
                    .icon(IconName::Users)
                    .label("Share")
                    .tooltip("Manage workspace access for this list")
                    .dropdown_menu(move |menu, _, _| {
                        let sa_owner = set_access.clone();
                        let sa_read = set_access.clone();
                        let sa_write = set_access.clone();
                        let sa_none = set_access.clone();
                        menu.item(
                            PopupMenuItem::new("Workspace: Owner (full control)").on_click(
                                move |_, window, cx| {
                                    sa_owner("owner".to_owned(), window, cx);
                                },
                            ),
                        )
                        .item(PopupMenuItem::new("Workspace: Can edit").on_click(
                            move |_, window, cx| {
                                sa_write("write".to_owned(), window, cx);
                            },
                        ))
                        .item(PopupMenuItem::new("Workspace: View only").on_click(
                            move |_, window, cx| {
                                sa_read("read".to_owned(), window, cx);
                            },
                        ))
                        .item(
                            PopupMenuItem::new("Restrict workspace access").on_click(
                                move |_, window, cx| {
                                    sa_none("none".to_owned(), window, cx);
                                },
                            ),
                        )
                    }),
            )
        })
        .when_some(actions.on_add_tab.clone(), |row, add_tab| {
            row.child(
                Button::new("list-add-tab")
                    .outline()
                    .with_size(Size::XSmall)
                    .icon(IconName::Plus)
                    .label("Pin tab")
                    .tooltip("Add this list as a conversation tab")
                    .on_click(move |_, window, cx| add_tab(window, cx)),
            )
        })
        .when_some(actions.on_delete_list.clone(), |row, delete_list| {
            row.child(
                Button::new("list-delete")
                    .outline()
                    .with_size(Size::XSmall)
                    .icon(IconName::Trash)
                    .tooltip("Delete this list")
                    .on_click(move |_, window, cx| delete_list(window, cx)),
            )
        })
        .when_some(actions.open_browser.clone(), |row, open| {
            row.child(
                Button::new("list-open-browser")
                    .outline()
                    .with_size(Size::XSmall)
                    .icon(IconName::ExternalLink)
                    .label("Open in browser")
                    .on_click(move |_, window, cx| open(window, cx)),
            )
        })
}
