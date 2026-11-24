// SPIKE ONLY (WS-00). Sphere impostors: one camera-facing quad per instance,
// ray-sphere intersection per fragment, exact depth written. The production
// pipelines belong to mold-render (WS-08).

struct Globals {
    view: mat4x4<f32>,
    proj: mat4x4<f32>,
    // x: index of the selected sphere, or -1.
    selected: vec4<i32>,
};

@group(0) @binding(0) var<uniform> globals: Globals;

struct Instance {
    @location(0) center_radius: vec4<f32>,
    @location(1) color: vec4<f32>,
};

struct VertexOut {
    @builtin(position) clip: vec4<f32>,
    // The quad corner in view space, so the fragment can cast its ray.
    @location(0) view_pos: vec3<f32>,
    @location(1) @interpolate(flat) center: vec3<f32>,
    @location(2) @interpolate(flat) radius: f32,
    @location(3) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, @builtin(instance_index) index: u32, instance: Instance) -> VertexOut {
    let corner = vec2<f32>(f32(vertex & 1u), f32((vertex >> 1u) & 1u)) * 2.0 - 1.0;
    let center = (globals.view * vec4<f32>(instance.center_radius.xyz, 1.0)).xyz;
    let radius = instance.center_radius.w;
    // In front of the centre by the radius and wide enough to cover the
    // silhouette from up close and off-axis (where it stretches, up to ~1.2x
    // at the corners of a 40° view); the fragment discards the rest.
    let front = center + vec3<f32>(0.0, 0.0, radius);
    let grow = length(center) / max(length(center) - radius, 1e-3);
    let view_pos = front + vec3<f32>(corner * radius * grow * 1.3, 0.0);

    var out: VertexOut;
    out.clip = globals.proj * vec4<f32>(view_pos, 1.0);
    out.view_pos = view_pos;
    out.center = center;
    out.radius = radius;
    out.color = instance.color;
    if (i32(index) == globals.selected.x) {
        out.color = vec4<f32>(0.2, 1.0, 0.45, 1.0);
    }
    return out;
}

struct FragmentOut {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

@fragment
fn fs_main(in: VertexOut) -> FragmentOut {
    // The eye is at the origin of view space.
    let dir = normalize(in.view_pos);
    let b = dot(dir, in.center);
    let c = dot(in.center, in.center) - in.radius * in.radius;
    let discriminant = b * b - c;
    if (discriminant < 0.0) {
        discard;
    }
    let t = b - sqrt(discriminant);
    let hit = dir * t;
    let normal = normalize(hit - in.center);

    // A headlight with a little rim, enough to read the shapes.
    let light = normalize(vec3<f32>(0.35, 0.5, 1.0));
    let diffuse = max(dot(normal, light), 0.0);
    let half_vector = normalize(light - dir);
    let specular = pow(max(dot(normal, half_vector), 0.0), 48.0) * 0.35;
    let ambient = 0.22;
    let rgb = in.color.rgb * (ambient + diffuse * 0.8) + vec3<f32>(specular);

    let clip = globals.proj * vec4<f32>(hit, 1.0);
    var out: FragmentOut;
    out.color = vec4<f32>(rgb, 1.0);
    out.depth = clip.z / clip.w;
    return out;
}
