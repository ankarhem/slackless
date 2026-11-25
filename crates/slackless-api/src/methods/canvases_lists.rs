//! Canvases, Slack Lists, file collections/favorites/trash, and profile photos.

use super::*;

impl SlackApi {
    /// Load a live Slack List (`files.info` for schema + `lists.records.list`, with public
    /// `slackLists.items.list` fallback) for read-only native table rendering.
    pub async fn fetch_list(&self, list_id: &str) -> Result<SlackListData> {
        let info_val: Value = self
            .call(
                method::FILES_INFO,
                vec![("file", list_id.to_owned()), ("count", "0".to_owned())],
            )
            .await?
            .data;
        let file_obj = info_val.get("file").unwrap_or(&info_val);
        let (mut title, mut description, mut columns) = parse_list_schema(file_obj);

        let private_res = self
            .call::<Value>(
                method::LISTS_RECORDS_LIST,
                vec![
                    ("list_id", list_id.to_owned()),
                    ("limit", "200".to_owned()),
                    ("archived", "false".to_owned()),
                ],
            )
            .await;

        let records_val = match private_res {
            Ok(resp) => resp.data,
            Err(_) => {
                self.call::<Value>(
                    method::SLACK_LISTS_ITEMS_LIST,
                    vec![
                        ("list_id", list_id.to_owned()),
                        ("limit", "200".to_owned()),
                        ("include_list", "true".to_owned()),
                    ],
                )
                .await?
                .data
            }
        };

        if columns.is_empty()
            && let Some(list_obj) = records_val.get("list")
        {
            let (t, d, cols) = parse_list_schema(list_obj);
            if !t.is_empty() {
                title = t;
            }
            if !d.is_empty() {
                description = d;
            }
            columns = cols;
        }

        let raw_records = records_val
            .get("records")
            .or_else(|| records_val.get("items"))
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or(&[]);
        let rows = parse_list_records(raw_records);

        Ok(SlackListData {
            file_id: list_id.to_owned(),
            title,
            description,
            columns,
            rows,
        })
    }

    /// `files.favorites.list`: starred file IDs, newest star first (`F-CORE-133`).
    pub async fn files_favorites_list(&self) -> Result<Vec<String>> {
        let wire: FileFavoritesWire = self
            .call(
                method::FILES_FAVORITES_LIST,
                vec![("type", "all".to_owned())],
            )
            .await?
            .data;
        let mut ids = wire.file_ids;
        ids.reverse();
        Ok(ids)
    }

    /// `files.favorites.add` / `.remove`: star/unstar a file or add/remove it from a collection.
    pub async fn files_favorites_set(
        &self,
        file_id: &str,
        collection_id: Option<&str>,
        starred: bool,
    ) -> Result<()> {
        let mut params = vec![
            ("file_id", file_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(cid) = collection_id.filter(|s| !s.is_empty()) {
            params.push(("collection_id", cid.to_owned()));
        }
        let endpoint = if starred {
            method::FILES_FAVORITES_ADD
        } else {
            method::FILES_FAVORITES_REMOVE
        };
        self.call::<Empty>(endpoint, params).await?;
        Ok(())
    }

    /// `files.collections.list`: custom Files sidebar sections (`F-CORE-133`).
    pub async fn files_collections_list(&self) -> Result<Vec<FileCollection>> {
        let wire: FileCollectionsListWire = self
            .call(method::FILES_COLLECTIONS_LIST, Vec::new())
            .await?
            .data;
        Ok(wire.collections.into_iter().map(Into::into).collect())
    }

    /// `files.collections.create`: create a custom Files sidebar section (`F-CORE-133`).
    pub async fn files_collections_create(
        &self,
        name: &str,
        emoji: Option<&str>,
    ) -> Result<Vec<FileCollection>> {
        let mut params = vec![
            ("name", name.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        if let Some(e) = emoji.filter(|s| !s.is_empty()) {
            params.push(("emoji", e.to_owned()));
        }
        let wire: FileCollectionsListWire = self
            .call(method::FILES_COLLECTIONS_CREATE, params)
            .await?
            .data;
        Ok(wire.collections.into_iter().map(Into::into).collect())
    }

    /// `files.collections.delete`: delete a custom Files sidebar section (`F-CORE-133`).
    pub async fn files_collections_delete(&self, collection_id: &str) -> Result<()> {
        let params = vec![
            ("collection", collection_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::FILES_COLLECTIONS_DELETE, params)
            .await?;
        Ok(())
    }

    /// `files.recentlyDeleted`: canvases & lists deleted in the last 24h (`F-CORE-133`).
    pub async fn files_recently_deleted(&self) -> Result<Vec<DeletedFile>> {
        let wire: RecentlyDeletedWire = self
            .call(method::FILES_RECENTLY_DELETED, Vec::new())
            .await?
            .data;
        Ok(wire
            .files
            .into_iter()
            .filter(|f| !f.id.is_empty())
            .map(|f| DeletedFile {
                id: f.id,
                title: if f.title.is_empty() {
                    "Untitled".to_owned()
                } else {
                    f.title
                },
                date_delete: f.date_delete,
                file_type: f.file_type,
            })
            .collect())
    }

    /// `files.undelete`: restore a recently deleted canvas or list (`F-CORE-133`).
    pub async fn files_undelete(&self, file_id: &str) -> Result<()> {
        let params = vec![
            ("file_id", file_id.to_owned()),
            ("_x_mode", MODE_ONLINE.to_owned()),
        ];
        self.call::<Empty>(method::FILES_UNDELETE, params).await?;
        Ok(())
    }

    /// `canvases.create`: create a standalone canvas or channel-tabbed canvas (`F-COLLAB-119`).
    pub async fn canvases_create(
        &self,
        title: Option<&str>,
        markdown: Option<&str>,
        channel_id: Option<&ChannelId>,
    ) -> Result<CanvasCreated> {
        let mut params = Vec::new();
        if let Some(t) = title.filter(|s| !s.is_empty()) {
            params.push(("title", t.to_owned()));
        }
        if let Some(md) = markdown.filter(|s| !s.is_empty()) {
            let content = serde_json::json!({ "type": "markdown", "markdown": md }).to_string();
            params.push(("document_content", content));
        }
        if let Some(ch) = channel_id {
            params.push(("channel_id", ch.to_string()));
        }
        Ok(self.call(method::CANVASES_CREATE, params).await?.data)
    }

    /// `canvases.edit`: apply one section-level edit operation to a canvas (`F-COLLAB-132`, `F-SHELL-264`, `F-SHELL-277`).
    pub async fn canvases_edit(&self, canvas_id: &str, op: &CanvasEditOp) -> Result<()> {
        let changes = serde_json::Value::Array(vec![op.to_change_json()]).to_string();
        let params = vec![("canvas_id", canvas_id.to_owned()), ("changes", changes)];
        self.call::<Empty>(method::CANVASES_EDIT, params).await?;
        Ok(())
    }

    /// `canvases.getContent`: fetch a canvas's markdown or HTML body (`canvases.getContent`).
    pub async fn canvases_get_content(
        &self,
        canvas_id: &str,
        content_type: Option<&str>,
    ) -> Result<String> {
        let mut params = vec![("canvas_id", canvas_id.to_owned())];
        if let Some(ct) = content_type.filter(|s| !s.is_empty()) {
            params.push(("content_type", ct.to_owned()));
        }
        let wire: CanvasContentWire = self.call(method::CANVASES_GET_CONTENT, params).await?.data;
        Ok(wire.content)
    }

    /// `canvases.delete`: permanently delete a canvas.
    pub async fn canvases_delete(&self, canvas_id: &str) -> Result<()> {
        let params = vec![("canvas_id", canvas_id.to_owned())];
        self.call::<Empty>(method::CANVASES_DELETE, params).await?;
        Ok(())
    }

    /// `files.requestPermission`: request edit (`upgrade`) or comment (`comment_upgrade`) access (`F-COLLAB-129`).
    pub async fn files_request_permission(&self, file_id: &str, request_type: &str) -> Result<()> {
        let params = vec![
            ("file_id", file_id.to_owned()),
            ("request_type", request_type.to_owned()),
            ("reason", "IndicatorRequestEditAccessButton".to_owned()),
        ];
        self.call::<Empty>(method::FILES_REQUEST_PERMISSION, params)
            .await?;
        Ok(())
    }

    /// `lists.create`: create a new Slack List from scratch (`F-COLLAB-213`).
    pub async fn lists_create(
        &self,
        name: &str,
        description: Option<&str>,
        channel_id: Option<&ChannelId>,
    ) -> Result<String> {
        let mut params = vec![("name", name.to_owned())];
        if let Some(desc) = description.filter(|s| !s.is_empty()) {
            params.push(("description", desc.to_owned()));
        }
        if let Some(ch) = channel_id {
            params.push(("context_channel_id", ch.to_string()));
        }
        let wire: ListCreateWire = self.call(method::LISTS_CREATE, params).await?.data;
        let id = wire
            .list_id
            .or_else(|| {
                wire.list
                    .as_ref()
                    .and_then(|v| v.get("id"))
                    .and_then(Value::as_str)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_default();
        Ok(id)
    }

    /// `lists.records.create` (with `slackLists.items.create` fallback): add an item to a List (`F-COLLAB-226`).
    pub async fn lists_records_create(
        &self,
        list_id: &str,
        initial_fields: &[ListCellInput],
        parent_record_id: Option<&str>,
    ) -> Result<SlackListRow> {
        let fields_json =
            Value::Array(initial_fields.iter().map(|c| c.to_json(None)).collect()).to_string();
        let mut params = vec![
            ("list_id", list_id.to_owned()),
            ("initial_fields", fields_json.clone()),
        ];
        if let Some(parent) = parent_record_id.filter(|s| !s.is_empty()) {
            params.push(("parent_record_id", parent.to_owned()));
        }
        let wire: ListRecordCreateWire = match self.call(method::LISTS_RECORDS_CREATE, params).await
        {
            Ok(resp) => resp.data,
            Err(_) => {
                let mut fallback = vec![
                    ("list_id", list_id.to_owned()),
                    ("initial_fields", fields_json),
                ];
                if let Some(parent) = parent_record_id.filter(|s| !s.is_empty()) {
                    fallback.push(("parent_item_id", parent.to_owned()));
                }
                self.call(method::SLACK_LISTS_ITEMS_CREATE, fallback)
                    .await?
                    .data
            }
        };
        let raw = wire.record.or(wire.item);
        let row = raw
            .as_ref()
            .and_then(|v| {
                parse_list_records(std::slice::from_ref(v))
                    .into_iter()
                    .next()
            })
            .unwrap_or_default();
        Ok(row)
    }

    /// `lists.cells.update` (with `slackLists.items.update` fallback): update one or more cells (`F-COLLAB-227`, `F-COLLAB-248`).
    pub async fn lists_cells_update(
        &self,
        list_id: &str,
        row_id: &str,
        cells: &[ListCellInput],
    ) -> Result<()> {
        let cells_json =
            Value::Array(cells.iter().map(|c| c.to_json(Some(row_id))).collect()).to_string();
        let params = vec![
            ("list_id", list_id.to_owned()),
            ("cells", cells_json.clone()),
        ];
        if self
            .call::<Empty>(method::LISTS_CELLS_UPDATE, params)
            .await
            .is_err()
        {
            let fallback = vec![("list_id", list_id.to_owned()), ("cells", cells_json)];
            self.call::<Empty>(method::SLACK_LISTS_ITEMS_UPDATE, fallback)
                .await?;
        }
        Ok(())
    }

    /// `lists.records.delete` (with `slackLists.items.delete` fallback): delete one list item (`F-COLLAB-228`).
    pub async fn lists_records_delete(&self, list_id: &str, row_id: &str) -> Result<()> {
        let params = vec![("list_id", list_id.to_owned()), ("id", row_id.to_owned())];
        if self
            .call::<Empty>(method::LISTS_RECORDS_DELETE, params.clone())
            .await
            .is_err()
        {
            self.call::<Empty>(method::SLACK_LISTS_ITEMS_DELETE, params)
                .await?;
        }
        Ok(())
    }

    /// `lists.addToList`: add a Slack message to an existing List (`F-COLLAB-231`).
    pub async fn lists_add_to_list(
        &self,
        list_id: &str,
        channel: &ChannelId,
        message_ts: &Ts,
    ) -> Result<String> {
        let params = vec![
            ("list_id", list_id.to_owned()),
            ("channel_id", channel.to_string()),
            ("message_ts", message_ts.to_string()),
            ("reason", "add-to-list".to_owned()),
        ];
        let wire: ListRecordCreateWire = self.call(method::LISTS_ADD_TO_LIST, params).await?.data;
        let id = wire
            .record
            .as_ref()
            .and_then(|v| v.get("id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        Ok(id)
    }

    /// `lists.records.archive`: archive one list item (`F-COLLAB-255`).
    pub async fn lists_records_archive(&self, list_id: &str, row_id: &str) -> Result<()> {
        let params = vec![
            ("list_id", list_id.to_owned()),
            ("record_id", row_id.to_owned()),
            ("id", row_id.to_owned()),
        ];
        self.call::<Empty>(method::LISTS_RECORDS_ARCHIVE, params)
            .await?;
        Ok(())
    }

    /// `slackLists.access.set`: update list sharing/access permissions (`F-COLLAB-243`–`245`).
    pub async fn lists_access_set(
        &self,
        list_id: &str,
        access_level: &str,
        channel: Option<&ChannelId>,
    ) -> Result<()> {
        let mut params = vec![
            ("list_id", list_id.to_owned()),
            ("access_level", access_level.to_owned()),
        ];
        if let Some(ch) = channel {
            params.push(("channel_ids", ch.to_string()));
        }
        let _: Value = self
            .call(method::SLACK_LISTS_ACCESS_SET, params)
            .await?
            .data;
        Ok(())
    }

    /// `lists.getThreadsForRecord`: load comment thread messages attached to a list item (`F-COLLAB-250`, `251`).
    pub async fn lists_get_threads_for_record(
        &self,
        list_id: &str,
        record_id: &str,
    ) -> Result<Vec<Message>> {
        let params = vec![
            ("list_id", list_id.to_owned()),
            ("record_id", record_id.to_owned()),
        ];
        let data: Value = self
            .call(method::LISTS_GET_THREADS_FOR_RECORD, params)
            .await?
            .data;
        let mut out = Vec::new();
        if let Some(msgs) = data
            .get("messages")
            .and_then(Value::as_array)
            .or_else(|| data.get("threads").and_then(Value::as_array))
        {
            for item in msgs {
                if let Ok(m) = serde_json::from_value::<Message>(item.clone()) {
                    out.push(m);
                }
            }
        }
        Ok(out)
    }

    /// `users.setPhoto`: upload a new profile photo (`F-SHELL-004`).
    pub async fn users_set_photo(&self, filename: &str, bytes: &[u8]) -> Result<()> {
        let lower = filename.to_ascii_lowercase();
        let ct = if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
            "image/jpeg"
        } else if lower.ends_with(".gif") {
            "image/gif"
        } else if lower.ends_with(".webp") {
            "image/webp"
        } else {
            "image/png"
        };
        let _ = self
            .post_multipart_file(method::USERS_SET_PHOTO, "image", filename, ct, bytes)
            .await?;
        Ok(())
    }
}
