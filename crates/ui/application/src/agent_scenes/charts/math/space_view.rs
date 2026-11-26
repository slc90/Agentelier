use super::{
    super::{
        axis::{AxisSpec, Range},
        construction, plot,
    },
    numeric_axis,
};
use agentelier_agent_host::{
    EcsError, MountedScene, SceneDefinition, SceneState, SceneStateWrites,
};
use bevy::{
    asset::RenderAssetUsages,
    camera::{RenderTarget, ScalingMode, primitives::MeshAabb, visibility::RenderLayers},
    math::DVec3,
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
    render::{render_resource::TextureFormat, view::Tonemapping},
    ui::{UiSystems, widget::ViewportNode},
};
use bevy_widgetry::{scene::spawn_scene, text::WidgetryText, theme::WidgetryTheme};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::{collections::BTreeSet, io};

#[derive(Clone, Copy, Component, Reflect, PartialEq, Deserialize, Serialize)]
#[reflect(Component)]
#[serde(deny_unknown_fields)]
pub(super) struct ViewAngles {
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Component)]
pub(super) struct SpaceView {
    camera: Entity,
    geometry: Entity,
    mesh: Handle<Mesh>,
    status: Entity,
    layer: usize,
}

#[derive(Component)]
struct SpaceViewport(Entity);

#[derive(Component)]
#[relationship(relationship_target = SpaceEntities)]
struct OwnedSpace(Entity);

#[derive(Component)]
#[relationship_target(relationship = OwnedSpace, linked_spawn)]
struct SpaceEntities(Vec<Entity>);

pub(super) struct Axes {
    pub limits: [[f64; 2]; 3],
    pub ranges: [Range; 3],
    names: [String; 3],
}

#[derive(Default)]
pub(super) struct Triangles {
    pub positions: Vec<[f32; 3]>,
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
}

pub(super) fn install(app: &mut App) {
    app.register_type::<ViewAngles>()
        .add_observer(rotate)
        .add_observer(save_rotation)
        .add_systems(PostUpdate, update_visibility.after(UiSystems::Layout));
}

impl Default for ViewAngles {
    fn default() -> Self {
        Self {
            yaw: 52.0,
            pitch: 34.0,
        }
    }
}

impl ViewAngles {
    pub fn validate(self) -> Result<Self> {
        if !self.yaw.is_finite() || !(-85.0..=85.0).contains(&self.pitch) {
            return Err(
                io::Error::other("视角需提供有限方位角 yaw，仰角 pitch 限于 −85° 至 85°").into(),
            );
        }
        Ok(Self {
            yaw: self.yaw.rem_euclid(360.0),
            ..self
        })
    }

    fn camera(self) -> Transform {
        let (y, x) = self.yaw.to_radians().sin_cos();
        let (height, radius) = self.pitch.to_radians().sin_cos();
        Transform::from_xyz(6.0 * radius * x, 6.0 * height, 6.0 * radius * y)
            .looking_at(Vec3::ZERO, Vec3::Y)
    }

    fn values(self) -> Map<String, Value> {
        Map::from_iter([
            ("yaw".into(), json!(self.yaw)),
            ("pitch".into(), json!(self.pitch)),
        ])
    }
}

impl Axes {
    pub fn new(specs: [AxisSpec<f64>; 3]) -> Result<Self> {
        let [x, y, z] = specs.map(|spec| {
            let limits = spec
                .range
                .ok_or_else(|| io::Error::other("三维数学图形需要显式 X、Y、Z 轴范围"))?;
            let axis = numeric_axis(spec, limits)?;
            Ok::<_, BevyError>((limits, axis))
        });
        let [x, y, z] = [x?, y?, z?];
        Ok(Self {
            limits: [x.0, y.0, z.0],
            ranges: [x.1.range, y.1.range, z.1.range],
            names: [x.1.name, y.1.name, z.1.name],
        })
    }

    pub fn matches(&self, other: &Self) -> bool {
        self.limits == other.limits && self.names == other.names
    }
}

pub(super) fn world_point(fractions: DVec3) -> Vec3 {
    let p = fractions.as_vec3() * 2.0 - Vec3::ONE;
    Vec3::new(p.x, p.z, -p.y)
}

impl SpaceView {
    pub fn mount(
        world: &mut World,
        parent: Entity,
        args: ViewMount<'_>,
    ) -> Result<Entity, EcsError> {
        let root = plot::root(world, parent, args.name, args.title)?;
        if let Err(error) = mount_content(world, root, args) {
            world.despawn(root);
            return Err(error);
        }
        Ok(root)
    }

    pub fn update(world: &mut World, root: Entity, geometry: Triangles, status: String) -> Result {
        let state = world
            .get::<Self>(root)
            .ok_or_else(|| io::Error::other("三维绘图缺少原生视口"))?;
        let (mesh, entity, label) = (state.mesh.clone(), state.geometry, state.status);
        let visibility = if geometry.indices.is_empty() {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
        let next = geometry.mesh();
        let bounds = next.get_aabb();
        *world
            .resource_mut::<Assets<Mesh>>()
            .get_mut(&mesh)
            .ok_or_else(|| io::Error::other("三维绘图缺少网格资源"))? = next;
        let mut rendered = world.entity_mut(entity);
        rendered.insert(visibility);
        if let Some(bounds) = bounds {
            rendered.insert(bounds);
        }
        world
            .get_mut::<Text>(label)
            .ok_or_else(|| io::Error::other("三维绘图缺少状态说明"))?
            .set_if_neq(Text(status));
        Ok(())
    }

    pub fn set_angles(world: &mut World, root: Entity, angles: ViewAngles) -> Result {
        let view = world
            .get::<Self>(root)
            .ok_or_else(|| io::Error::other("三维绘图缺少相机"))?;
        let camera = view.camera;
        world
            .get_mut::<Transform>(camera)
            .ok_or_else(|| io::Error::other("三维绘图缺少相机姿态"))?
            .set_if_neq(angles.camera());
        world.entity_mut(root).insert(angles);
        Ok(())
    }
}

pub(super) struct ViewMount<'a> {
    pub title: Option<&'a str>,
    pub name: &'static str,
    pub axes: &'a Axes,
    pub initial: ViewAngles,
    pub geometry: Triangles,
    pub status: String,
}

fn mount_content(world: &mut World, root: Entity, args: ViewMount<'_>) -> Result<(), EcsError> {
    let used: BTreeSet<_> = world
        .query::<&SpaceView>()
        .iter(world)
        .map(|state| state.layer)
        .collect();
    let layer = (2..)
        .find(|index| !used.contains(index))
        .ok_or_else(|| construction("三维图形没有可用渲染层"))?;
    let layers = RenderLayers::none().with(layer);
    let background = world
        .resource::<WidgetryTheme>()
        .text_field
        .editable
        .normal
        .background
        .with_alpha(1.0);
    let image = world
        .resource_mut::<Assets<Image>>()
        .add(Image::new_target_texture(
            1,
            1,
            TextureFormat::Bgra8UnormSrgb,
            None,
        ));
    let material = world
        .resource_mut::<Assets<StandardMaterial>>()
        .add(StandardMaterial {
            unlit: true,
            double_sided: true,
            cull_mode: None,
            ..default()
        });
    let camera = world
        .spawn((
            OwnedSpace(root),
            Name::new("MathSpaceCamera"),
            Camera3d::default(),
            Tonemapping::None,
            Camera {
                order: -100,
                clear_color: background.into(),
                ..default()
            },
            Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::FixedVertical {
                    viewport_height: 4.35,
                },
                ..OrthographicProjection::default_3d()
            }),
            RenderTarget::Image(image.into()),
            args.initial.camera(),
            layers.clone(),
        ))
        .id();
    let mut axes = Triangles::default();
    append_axes(&mut axes);
    let axis_mesh = world.resource_mut::<Assets<Mesh>>().add(axes.mesh());
    world.spawn((
        OwnedSpace(root),
        Name::new("MathSpaceAxes"),
        Mesh3d(axis_mesh),
        MeshMaterial3d(material.clone()),
        layers.clone(),
    ));
    let visible = !args.geometry.indices.is_empty();
    let mesh = world
        .resource_mut::<Assets<Mesh>>()
        .add(args.geometry.mesh());
    let geometry = world
        .spawn((
            OwnedSpace(root),
            Name::new("MathSpaceGeometry"),
            Mesh3d(mesh.clone()),
            MeshMaterial3d(material),
            layers,
            if visible {
                Visibility::Inherited
            } else {
                Visibility::Hidden
            },
        ))
        .id();
    let heading = args.title.map(|title| plot::heading(title.to_owned()));
    let scale_labels: Vec<_> = args
        .axes
        .names
        .iter()
        .zip(args.axes.limits)
        .zip(AXIS_COLORS)
        .zip(['X', 'Y', 'Z'])
        .map(|(((name, [min, max]), color), axis)| {
            let legend = plot::Legend {
                name: format!("{axis} · {name}  {min} → {max}"),
                color,
            };
            plot::legend(legend, 10.0, 10.0)
        })
        .collect();
    let scale_labels = bevy::scene::RelatedScenes::<Children>::new(scale_labels);
    spawn_scene(world, bsn! {
        ChildOf(root) Node { width: percent(100), min_width: px(0), flex_direction: FlexDirection::Column, row_gap: px(6) }
        Children [
            @{heading}--
            Pickable::default() Name("MathSpaceViewport")
            template(move |_| Ok(SpaceViewport(root))) template(move |_| Ok(ViewportNode::new(camera)))
            Node { width: percent(100), min_width: px(0), min_height: px(145), aspect_ratio: {Some(1.7)}, overflow: Overflow::clip() }
            --
            Node { min_width: px(0), column_gap: px(14), row_gap: px(4), flex_wrap: FlexWrap::Wrap } @scale_labels
        ]
    }).map_err(construction)?;
    let status_text = args.status;
    let status = spawn_scene(world, bsn! {
        ChildOf(root) Name("MathSpaceStatus")
        Text(status_text) WidgetryText TextLayout::linebreak(bevy::text::LineBreak::WordOrCharacter)
    }).map_err(construction)?;
    world.entity_mut(root).insert((
        args.initial,
        SpaceView {
            camera,
            geometry,
            mesh,
            status,
            layer,
        },
    ));
    Ok(())
}

const AXIS_COLORS: [Color; 3] = [
    Color::srgb_u8(155, 70, 53),
    Color::srgb_u8(38, 115, 82),
    Color::srgb_u8(49, 105, 171),
];

fn append_axes(mesh: &mut Triangles) {
    let base = world_point(DVec3::ZERO);
    let axes = [Vec3::X, Vec3::NEG_Z, Vec3::Y];
    let grid = Color::srgb_u8(174, 188, 180);
    for step in 0..=4 {
        let offset = step as f32 * 0.5;
        mesh.bar(
            base + Vec3::NEG_Z * offset,
            base + Vec3::NEG_Z * offset + Vec3::X * 2.0,
            0.006,
            grid,
        );
        mesh.bar(
            base + Vec3::X * offset,
            base + Vec3::X * offset - Vec3::Z * 2.0,
            0.006,
            grid,
        );
    }
    for (direction, color) in axes.into_iter().zip(AXIS_COLORS) {
        let end = base + direction * 2.15;
        mesh.arrow(base, end, 0.025, color);
        let tangent = direction.any_orthonormal_vector();
        for step in 0..=4 {
            let at = base + direction * (step as f32 * 0.5);
            mesh.bar(at - tangent * 0.055, at + tangent * 0.055, 0.015, color);
        }
    }
}

impl Triangles {
    pub fn push(&mut self, vertices: [Vec3; 3], colors: [[f32; 4]; 3]) {
        let start = self.positions.len() as u32;
        self.positions
            .extend(vertices.map(|point| point.to_array()));
        self.colors.extend(colors);
        self.indices.extend([start, start + 1, start + 2]);
    }

    pub fn bar(&mut self, start: Vec3, end: Vec3, width: f32, color: Color) {
        let direction = (end - start).normalize_or_zero();
        if direction == Vec3::ZERO {
            return;
        }
        let a = direction.any_orthonormal_vector() * (width * 0.5);
        let b = direction.cross(a);
        let corners = [a + b, a - b, -a - b, -a + b];
        let color = [color.to_linear().to_f32_array(); 3];
        for index in 0..4 {
            let a = corners[index];
            let b = corners[(index + 1) % 4];
            self.push([start + a, start + b, end + a], color);
            self.push([start + b, end + b, end + a], color);
        }
    }

    pub fn arrow(&mut self, start: Vec3, end: Vec3, width: f32, color: Color) {
        let direction = (end - start).normalize_or_zero();
        if direction == Vec3::ZERO {
            return;
        }
        let head = (end - start).length().min(width * 10.0) * 0.72;
        let neck = end - direction * head;
        self.bar(start, neck, width, color);
        let a = direction.any_orthonormal_vector() * head * 0.34;
        let b = direction.cross(a);
        let corners = [a, b, -a, -b];
        let color = [color.to_linear().to_f32_array(); 3];
        for i in 0..4 {
            self.push([neck + corners[i], neck + corners[(i + 1) % 4], end], color);
        }
    }

    fn mesh(self) -> Mesh {
        let normals = vec![[0.0, 1.0, 0.0]; self.positions.len()];
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.colors)
        .with_inserted_indices(Indices::U32(self.indices))
    }
}

fn rotate(
    mut event: On<PointerDrag>,
    ports: Query<&SpaceViewport>,
    mut views: Query<(&SpaceView, &mut ViewAngles)>,
    mut transforms: Query<&mut Transform>,
) {
    if event.button != PointerButton::Primary {
        return;
    }
    let Ok(port) = ports.get(event.entity) else {
        return;
    };
    let Ok((view, mut angles)) = views.get_mut(port.0) else {
        return;
    };
    let Ok(mut transform) = transforms.get_mut(view.camera) else {
        return;
    };
    angles.yaw = (angles.yaw + event.delta.x * 0.55).rem_euclid(360.0);
    angles.pitch = (angles.pitch + event.delta.y * 0.42).clamp(-85.0, 85.0);
    *transform = angles.camera();
    event.propagate(false);
}

fn save_rotation(event: On<PointerDragEnd>, ports: Query<&SpaceViewport>, mut commands: Commands) {
    if event.button != PointerButton::Primary {
        return;
    }
    let Ok(port) = ports.get(event.entity) else {
        return;
    };
    let root = port.0;
    commands.queue(move |world: &mut World| {
        if let Some(&mount) = world.get::<MountedScene>(root) {
            let Some(values) = snapshot(world, root) else {
                return;
            };
            let state = SceneState {
                revision: mount.revision,
                values,
                part_values: Default::default(),
            };
            world
                .resource_mut::<SceneStateWrites>()
                .schedule(root, mount, state);
        } else if let Some(owner) = world
            .get::<ChildOf>(root)
            .and_then(|area| world.get::<ChildOf>(area.parent()))
            .map(ChildOf::parent)
        {
            crate::agent_scenes::composition::schedule_save(world, owner);
        }
    });
}

fn update_visibility(
    ports: Query<(&SpaceViewport, &ComputedNode, &InheritedVisibility)>,
    views: Query<&SpaceView>,
    mut cameras: Query<&mut Camera>,
) {
    for (port, layout, visible) in &ports {
        if let Ok(view) = views.get(port.0)
            && let Ok(camera) = cameras.get_mut(view.camera)
        {
            camera
                .map_unchanged(|camera| &mut camera.is_active)
                .set_if_neq(visible.get() && layout.size().min_element() > 0.0);
        }
    }
}

pub(in crate::agent_scenes) fn snapshot(world: &World, root: Entity) -> Option<Map<String, Value>> {
    world.get::<ViewAngles>(root).map(|angles| angles.values())
}

pub(super) fn orientation(values: &Map<String, Value>) -> Result<ViewAngles> {
    serde_json::from_value::<ViewAngles>(Value::Object(values.clone()))?.validate()
}

pub(in crate::agent_scenes) fn restore_definition(
    scene: &mut SceneDefinition,
    values: &Map<String, Value>,
) -> Result {
    if supports_orientation(scene) {
        scene
            .props
            .insert("view".into(), serde_json::to_value(orientation(values)?)?);
    }
    Ok(())
}

pub(in crate::agent_scenes) fn supports_orientation(scene: &SceneDefinition) -> bool {
    scene.template == super::surface::TEMPLATE
        || (scene.template == super::vector_field::TEMPLATE
            && scene
                .props
                .get("z_axis")
                .is_some_and(|axis| !axis.is_null()))
}
