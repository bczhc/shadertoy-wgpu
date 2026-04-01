@vertex
fn vs_main(@builtin(vertex_index) idx: u32) -> @builtin(position) vec4f {
    let pos = array(
        vec2f(-1, -1),
        vec2f(-1, 1),
        vec2f(1, 1),
        vec2f(1, 1),
        vec2f(1, -1),
        vec2f(-1, -1),
    );
    return vec4f(pos[idx], 0.0, 1.0);
}