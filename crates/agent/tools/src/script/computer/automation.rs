use super::{ScriptResult, fields, with_run};
use crate::ToolFailure;
use crate::desktop::uia::{RootSummary, TreePage, TreeRequest};
use rhai::{Array, Dynamic, Engine, INT, ImmutableString, Map, NativeCallContext};

pub(super) fn register(engine: &mut Engine) {
    engine
        .register_fn("uia_status", status)
        .register_fn("uia_desktop_root", desktop_root)
        .register_fn("uia_window_root", window_root)
        .register_fn("uia_root_info", root_info)
        .register_fn("uia_tree", default_tree)
        .register_fn("uia_tree", tree)
        .register_fn("uia_tree_related", tree_related);
}

fn status(ctx: NativeCallContext<'_>) -> ScriptResult<bool> {
    with_run(ctx, |run| {
        run.uia.status(&run.control.cancel, run.control.deadline)
    })
}

fn desktop_root(ctx: NativeCallContext<'_>) -> ScriptResult<Map> {
    with_run(ctx, |run| {
        run.uia
            .desktop_root(&run.control.cancel, run.control.deadline)
            .map(root_fields)
    })
}

fn window_root(ctx: NativeCallContext<'_>, window_id: ImmutableString) -> ScriptResult<Map> {
    with_run(ctx, |run| {
        run.uia
            .window_root(&window_id, &run.control.cancel, run.control.deadline)
            .map(root_fields)
    })
}

fn root_info(ctx: NativeCallContext<'_>, reference: ImmutableString) -> ScriptResult<Map> {
    with_run(ctx, |run| {
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
    with_run(ctx, |run| {
        run.uia
            .tree(
                TreeRequest::for_root(&reference)?,
                &run.control.cancel,
                run.control.deadline,
            )
            .map(tree_fields)
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
    with_run(ctx, |run| {
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
            .map(tree_fields)
    })
}

fn tree_related(
    ctx: NativeCallContext<'_>,
    reference: ImmutableString,
    view: ImmutableString,
    relation: ImmutableString,
) -> ScriptResult<Dynamic> {
    with_run(ctx, |run| {
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

fn tree_fields(page: TreePage) -> Map {
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
    let issue = page.issue.map_or(Dynamic::UNIT, |issue| {
        fields([
            ("code", issue.cause.code.into()),
            ("message", issue.cause.to_string().into()),
            ("parent", issue.parent.map_or(Dynamic::UNIT, Dynamic::from)),
            ("depth", (issue.depth as INT).into()),
        ])
        .into()
    });
    fields([
        ("view", page.view.name().into()),
        ("root", page.root.into()),
        ("generation", page.generation.into()),
        ("count", (nodes.len() as INT).into()),
        ("nodes", nodes.into()),
        ("has_more", page.cursor.is_some().into()),
        ("cursor", page.cursor.map_or(Dynamic::UNIT, Dynamic::from)),
        ("error", issue),
    ])
}
