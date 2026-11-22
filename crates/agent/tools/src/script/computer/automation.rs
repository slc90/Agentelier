use super::{Run, ScriptResult, fields, with_run};
use crate::ToolFailure;
use crate::desktop::uia::{
    BasicAction, CacheRequest, Expansion, FindRequest, ItemLookup, RootSummary, ScrollAmounts,
    SelectionChange, TreePage, TreeRequest,
};
use rhai::{Array, Dynamic, Engine, FLOAT, INT, ImmutableString, Map, NativeCallContext};

mod events;
mod grid;
mod interaction;
mod metadata;
mod snapshot;
mod text;
mod transform;
mod window;

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn("uia_status", status)
        .register_fn("uia_desktop_root", desktop_root)
        .register_fn("uia_window_root", window_root)
        .register_fn("uia_root_info", root_info)
        .register_fn("uia_tree", default_tree)
        .register_fn("uia_tree", tree)
        .register_fn("uia_tree_related", tree_related)
        .register_fn("uia_properties", properties)
        .register_fn("uia_cached_properties", cached_properties)
        .register_fn("uia_prefetch", prefetch)
        .register_fn("uia_patterns", patterns)
        .register_fn("uia_find", find)
        .register_fn("uia_ref_info", ref_info)
        .register_fn("uia_refresh_element", refresh_element)
        .register_fn(
            "uia_invoke",
            |ctx: NativeCallContext, id: ImmutableString| basic(ctx, id, Ok(BasicAction::Invoke)),
        )
        .register_fn(
            "uia_value_get",
            |ctx: NativeCallContext, id: ImmutableString| basic(ctx, id, Ok(BasicAction::ValueGet)),
        )
        .register_fn(
            "uia_value_set",
            |ctx: NativeCallContext, id: ImmutableString, value: ImmutableString| {
                basic(ctx, id, Ok(BasicAction::ValueSet(value.into_owned())))
            },
        )
        .register_fn(
            "uia_toggle",
            |ctx: NativeCallContext, id: ImmutableString| {
                basic(ctx, id, Ok(BasicAction::Toggle { change: true }))
            },
        )
        .register_fn(
            "uia_toggle",
            |ctx: NativeCallContext, id: ImmutableString, change: bool| {
                basic(ctx, id, Ok(BasicAction::Toggle { change }))
            },
        )
        .register_fn(
            "uia_range_value_get",
            |ctx: NativeCallContext, id: ImmutableString| basic(ctx, id, Ok(BasicAction::RangeGet)),
        )
        .register_fn(
            "uia_range_value_set",
            |ctx: NativeCallContext, id: ImmutableString, value: FLOAT| {
                basic(ctx, id, Ok(BasicAction::RangeSet(value)))
            },
        )
        .register_fn(
            "uia_range_value_set",
            |ctx: NativeCallContext, id: ImmutableString, value: INT| {
                basic(ctx, id, Ok(BasicAction::RangeSet(value as FLOAT)))
            },
        )
        .register_fn(
            "uia_selection",
            |ctx: NativeCallContext, id: ImmutableString| {
                basic(ctx, id, Ok(BasicAction::Selection))
            },
        )
        .register_fn(
            "uia_select",
            |ctx: NativeCallContext, id: ImmutableString, action: ImmutableString| {
                basic(
                    ctx,
                    id,
                    action.parse::<SelectionChange>().map(BasicAction::Select),
                )
            },
        )
        .register_fn(
            "uia_expand_collapse",
            |ctx: NativeCallContext, id: ImmutableString, action: ImmutableString| {
                basic(
                    ctx,
                    id,
                    action.parse::<Expansion>().map(BasicAction::ExpandCollapse),
                )
            },
        )
        .register_fn("uia_scroll", scroll_state)
        .register_fn("uia_scroll", scroll)
        .register_fn("uia_scroll_item", scroll_item)
        .register_fn("uia_find_item", find_first_item)
        .register_fn("uia_find_item", find_item)
        .register_fn("uia_realize", realize);
    text::register(engine);
    events::register(engine);
    grid::register(engine);
    window::register(engine);
    transform::register(engine);
    interaction::register(engine);
    metadata::register(engine);
    snapshot::register(engine);
}

fn scroll_state(ctx: NativeCallContext<'_>, id: ImmutableString) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .scroll(&id, None, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn scroll(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    horizontal: ImmutableString,
    vertical: ImmutableString,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        let amounts = ScrollAmounts::new(&horizontal, &vertical)?;
        run.uia
            .scroll(
                &id,
                Some(amounts),
                &run.control.cancel,
                run.control.deadline,
            )
            .and_then(plain_data)
    })
}

fn scroll_item(ctx: NativeCallContext<'_>, id: ImmutableString) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .scroll_item(&id, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn find_first_item(
    ctx: NativeCallContext<'_>,
    container: ImmutableString,
    property: ImmutableString,
    value: ImmutableString,
) -> ScriptResult<Map> {
    find_in_container(ctx, container, property, value, None)
}

fn find_item(
    ctx: NativeCallContext<'_>,
    container: ImmutableString,
    property: ImmutableString,
    value: ImmutableString,
    after: ImmutableString,
) -> ScriptResult<Map> {
    find_in_container(ctx, container, property, value, Some(after))
}

fn find_in_container(
    ctx: NativeCallContext<'_>,
    container: ImmutableString,
    property: ImmutableString,
    value: ImmutableString,
    after: Option<ImmutableString>,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        let lookup = ItemLookup::new(&property, value.into_owned())?;
        run.uia
            .find_item(
                &container,
                lookup,
                after.as_ref().map(ImmutableString::as_str),
                &run.control.cancel,
                run.control.deadline,
            )
            .and_then(plain_data)
    })
}

fn realize(ctx: NativeCallContext<'_>, id: ImmutableString) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .realize(&id, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn basic(
    ctx: NativeCallContext<'_>,
    id: ImmutableString,
    action: Result<BasicAction, ToolFailure>,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .basic_pattern(&id, action?, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn status(ctx: NativeCallContext<'_>) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia.status(&run.control.cancel, run.control.deadline)
    })
}

fn desktop_root(ctx: NativeCallContext<'_>) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .desktop_root(&run.control.cancel, run.control.deadline)
            .map(root_fields)
    })
}

fn window_root(ctx: NativeCallContext<'_>, window_id: ImmutableString) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .window_root(&window_id, &run.control.cancel, run.control.deadline)
            .map(root_fields)
    })
}

fn root_info(ctx: NativeCallContext<'_>, reference: ImmutableString) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .root_info(&reference, &run.control.cancel, run.control.deadline)
            .map(root_fields)
    })
}

fn root_fields(summary: RootSummary) -> Map {
    fields([
        ("ref", summary.reference.into()),
        (
            "window_id",
            summary.window_id.map_or(Dynamic::UNIT, Dynamic::from),
        ),
        ("name", summary.name.into()),
        ("class_name", summary.class_name.into()),
        ("control_type", summary.control_type.into()),
        ("provider_accessible", true.into()),
    ])
}

fn default_tree(ctx: NativeCallContext<'_>, reference: ImmutableString) -> ScriptResult<Map> {
    with_reply(ctx, Source::Live, |run| {
        run.uia
            .tree(
                TreeRequest::for_root(&reference)?,
                &run.control.cancel,
                run.control.deadline,
            )
            .map(tree_reply)
    })
}

fn tree(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    view: ImmutableString,
    max_depth: INT,
    max_nodes: INT,
    cursor: Dynamic,
) -> ScriptResult<Map> {
    with_reply(ctx, Source::Live, |run| {
        let cursor =
            if cursor.is_unit() {
                None
            } else {
                Some(cursor.try_cast::<ImmutableString>().ok_or_else(|| {
                    ToolFailure::new("uia_invalid_argument", "树游标须为字符串或 ()")
                })?)
            };
        let request = TreeRequest::new(
            &reference,
            &view,
            max_depth,
            max_nodes,
            cursor.as_ref().map(ImmutableString::as_str),
        )?;
        run.uia
            .tree(request, &run.control.cancel, run.control.deadline)
            .map(tree_reply)
    })
}

fn tree_related(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    view: ImmutableString,
    relation: ImmutableString,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        let related = run.uia.tree_related(
            &reference,
            view.parse()?,
            relation.parse()?,
            &run.control.cancel,
            run.control.deadline,
        )?;
        Ok(related.map_or(Dynamic::UNIT, |summary| root_fields(summary).into()))
    })
}

fn tree_reply(page: TreePage) -> Reply {
    let nodes = page
        .nodes
        .into_iter()
        .map(|node| {
            let mut map = root_fields(node.summary);
            map.insert(
                "parent".into(),
                node.parent.map_or(Dynamic::UNIT, Dynamic::from),
            );
            map.insert("depth".into(), (node.depth as INT).into());
            map.into()
        })
        .collect::<Array>();
    let issue_details = page.issue.as_ref().map_or(Dynamic::UNIT, |issue| {
        fields([
            (
                "parent",
                issue
                    .parent
                    .as_ref()
                    .map_or(Dynamic::UNIT, |parent| Dynamic::from(parent.clone())),
            ),
            ("depth", (issue.depth as INT).into()),
        ])
        .into()
    });
    let data = fields([
        ("view", page.view.name().into()),
        ("root", page.root.into()),
        ("generation", page.generation.into()),
        ("count", (nodes.len() as INT).into()),
        ("nodes", nodes.into()),
        ("has_more", page.cursor.is_some().into()),
        ("cursor", page.cursor.map_or(Dynamic::UNIT, Dynamic::from)),
        ("issue", issue_details),
    ]);
    Reply {
        data: data.into(),
        issue: page.issue.map(|issue| issue.cause),
        generation: None,
    }
}

fn properties(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    names: Array,
) -> ScriptResult<Map> {
    read_properties(ctx, reference, names, false)
}

fn cached_properties(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    names: Array,
) -> ScriptResult<Map> {
    read_properties(ctx, reference, names, true)
}

fn read_properties(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    names: Array,
    cached: bool,
) -> ScriptResult<Map> {
    let source = if cached { Source::Cache } else { Source::Live };
    with_reply(ctx, source, |run| {
        run.uia
            .properties(
                &reference,
                &strings(names)?,
                cached,
                &run.control.cancel,
                run.control.deadline,
            )
            .and_then(plain_data)
            .map(Reply::data)
    })
}

fn prefetch(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    property_names: Array,
    pattern_names: Array,
    view: ImmutableString,
    scope: ImmutableString,
) -> ScriptResult<Map> {
    with_reply(ctx, Source::Cache, |run| {
        let request = CacheRequest::new(
            &strings(property_names)?,
            &strings(pattern_names)?,
            &view,
            &scope,
        )?;
        run.uia
            .prefetch(
                &reference,
                request,
                &run.control.cancel,
                run.control.deadline,
            )
            .and_then(plain_data)
            .map(Reply::data)
    })
}

fn patterns(ctx: NativeCallContext<'_>, reference: ImmutableString) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .patterns(&reference, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn ref_info(ctx: NativeCallContext<'_>, reference: ImmutableString) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .ref_info(&reference, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn refresh_element(ctx: NativeCallContext<'_>, reference: ImmutableString) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        run.uia
            .refresh_element(&reference, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn find(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    conditions: Dynamic,
    view: ImmutableString,
    scope: ImmutableString,
    max_depth: INT,
    max_nodes: INT,
    max_results: INT,
) -> ScriptResult<Map> {
    with_uia(ctx, |run| {
        let conditions = rhai::serde::from_dynamic(&conditions)
            .map_err(|error| ToolFailure::new("uia_invalid_argument", error.to_string()))?;
        let request = FindRequest::new(
            &reference,
            conditions,
            &view,
            &scope,
            max_depth,
            max_nodes,
            max_results,
        )?;
        run.uia
            .find(request, &run.control.cancel, run.control.deadline)
            .and_then(plain_data)
    })
}

fn strings(items: Array) -> Result<Vec<String>, ToolFailure> {
    items
        .into_iter()
        .map(|item| {
            item.try_cast::<ImmutableString>()
                .map(|name| name.into_owned())
                .ok_or_else(|| ToolFailure::new("uia_invalid_argument", "名称列表的值须为字符串"))
        })
        .collect()
}

fn plain_data(value: serde_json::Value) -> Result<Dynamic, ToolFailure> {
    rhai::serde::to_dynamic(value)
        .map_err(|error| ToolFailure::new("uia_result_conversion_failed", error.to_string()))
}

#[derive(Clone, Copy)]
enum Source {
    Live,
    Cache,
    Snapshot,
}

struct Reply {
    data: Dynamic,
    issue: Option<ToolFailure>,
    generation: Option<INT>,
}

impl Reply {
    fn data(data: impl Into<Dynamic>) -> Self {
        Self {
            data: data.into(),
            issue: None,
            generation: None,
        }
    }

    fn snapshot(result: crate::desktop::uia::SnapshotResult) -> Result<Self, ToolFailure> {
        Ok(Self {
            data: plain_data(result.data)?,
            issue: None,
            generation: Some(result.generation),
        })
    }

    fn into_fields(self, source: Source) -> Map {
        let error = self.issue.as_ref().map_or(Dynamic::UNIT, |error| {
            fields([
                ("code", error.code.into()),
                ("message", error.to_string().into()),
            ])
            .into()
        });
        let source = match source {
            Source::Live => "live",
            Source::Cache => "cache",
            Source::Snapshot => "snapshot",
        };
        fields([
            ("ok", self.issue.is_none().into()),
            ("data", self.data),
            ("error", error),
            ("source", source.into()),
            (
                "generation",
                self.generation.map_or(Dynamic::UNIT, Dynamic::from),
            ),
        ])
    }
}

fn with_uia<T: Into<Dynamic>>(
    ctx: NativeCallContext<'_>,
    call: impl FnOnce(&mut Run) -> Result<T, ToolFailure>,
) -> ScriptResult<Map> {
    with_reply(ctx, Source::Live, |run| call(run).map(Reply::data))
}

fn with_reply(
    ctx: NativeCallContext<'_>,
    source: Source,
    call: impl FnOnce(&mut Run) -> Result<Reply, ToolFailure>,
) -> ScriptResult<Map> {
    with_run(ctx, |run| {
        let reply = match call(run) {
            Ok(reply) => reply,
            Err(error)
                if matches!(error.code, "uia_cancelled" | "uia_timeout")
                    && run.control.check().is_err() =>
            {
                return Err(error);
            }
            Err(error) => Reply {
                data: Dynamic::UNIT,
                issue: Some(error),
                generation: None,
            },
        };
        Ok(reply.into_fields(source))
    })
}
