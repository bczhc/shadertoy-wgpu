#version 450

layout(location = 0) out vec4 renderOut;
layout(set = 0, binding = 0) uniform vec3 iResolution;
layout(set = 0, binding = 1) uniform float iTime;
layout(set = 0, binding = 2) uniform vec4 iMouse;
layout(set = 0, binding = 3) uniform int iFrame;

MAIN_IMAGE;

void main() {
    // Prevent code-not-used elimination.
    float _1 = iResolution.x;
    float _2 = iTime;
    float _3 = iMouse.x;
    int _4 = iFrame;

    vec4 t;
    mainImage(t, vec2(gl_FragCoord.x, iResolution.y - gl_FragCoord.y));
    renderOut = t;
}
