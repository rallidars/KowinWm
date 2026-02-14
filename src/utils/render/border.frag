//precision mediump float;
//// The size or dimensions.
//uniform vec2 u_resolution;
//// Color of border.
//uniform vec3 border_color;
//// Thickness of border.
//uniform float border_thickness;
//// The ratio of the coordinate to the resolution.
//varying vec2 v_coords;
//
//void main() {
//    // Get the pixel coordinates.
//    vec2 coords = v_coords * u_resolution;
//
//    // Step function is just (param1 < param2) return 1.0 for true and 0.0 for false.
//    // On the left side, if the coordinate is less than the thickness, draw a border.
//    float xl = step(coords.x, border_thickness);
//    float yl = step(coords.y, border_thickness);
//    // On the right side, if (coordinate - border_thickness) is less than the coordinate, draw a border.
//    float xr = step(u_resolution.x - border_thickness, coords.x);
//    float yr = step(u_resolution.y - border_thickness, coords.y);
//
//    // The alpha will become 1.0 or greater if any of the above statements are true.
//    float alpha = xl + yl + xr + yr;
//
//    gl_FragColor = vec4(border_color * alpha, alpha);
//}
precision mediump float;
uniform vec2 size;
varying vec2 v_coords;

uniform vec2 gradientDirection;
uniform vec3 startColor;
uniform vec3 endColor;
uniform float thickness;
uniform float halfThickness;

void main() {
    vec2 center = size / 2.0 - vec2(0.5);
    vec2 location = v_coords * size;
    vec4 mix_color;

    float distance = max(abs(location.x - center.x) - (size.x / 2.0 - halfThickness), abs(location.y - center.y) - (size.y / 2.0 - halfThickness));
    float smoothedAlpha = 1.0 - smoothstep(0.0, 1.0, abs(distance) - (halfThickness));

    float dotProduct = dot(v_coords, gradientDirection);

    vec3 gradientColor = mix(startColor, endColor, smoothstep(0.0, 1.0, dotProduct));

    mix_color = mix(vec4(0.0, 0.0, 0.0, 0.0), vec4(gradientColor, smoothedAlpha), smoothedAlpha);

    gl_FragColor = mix_color;
}
