#version 330


in vec2 inPos;
in vec2 inTex;

out gl_PerVertex
{
	vec4 gl_Position;
};
out vec2 fsTex;

uniform mat4 proj;
uniform mat4 world;

void main()
{
	fsTex = inTex;
	gl_Position = proj * world * vec4(inPos.xy, 0, 1);
}