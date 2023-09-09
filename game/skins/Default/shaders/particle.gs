
in;
out;

// Input
in gl_PerVertex
{
	vec4 gl_Position;
} gl_in[1];
in vec4 inColor[1];
in vec4 inParams[1];

// Output
out gl_PerVertex
{
	vec4 gl_Position;
};
out vec4 fsColor;
out vec2 fsTex;

uniform mat4 proj;
uniform mat4 camera;
uniform mat4 billboard;

void main()
{
	float fScale = inParams[0].x;
	float fRotation = inParams[0].y;
	float fAnimationFrame = inParams[0].z;
	
	vec2 rightAxis2D = vec2(cos(fRotation), sin(fRotation));
	vec2 upAxis2D = vec2(-sin(fRotation), cos(fRotation));
	vec4 cameraRight = billboard * vec4(rightAxis2D, 0, 0);
	vec4 cameraUp = billboard * vec4(upAxis2D, 0, 0);
	
	gl_Position = proj * camera * (gl_in[0].gl_Position + (-cameraRight - cameraUp) * fScale);
	fsColor = inColor[0];
	fsTex = vec2(0.0f, 0.0f);
	EmitVertex();

	gl_Position = proj * camera * (gl_in[0].gl_Position + (cameraRight - cameraUp) * fScale);
	fsTex = vec2(1.0f, 0.0f);
	EmitVertex();
	
	gl_Position = proj * camera * (gl_in[0].gl_Position + (-cameraRight + cameraUp) * fScale);
	fsTex = vec2(0.0f, 1.0f);
	EmitVertex();

	gl_Position = proj * camera * (gl_in[0].gl_Position + (cameraRight + cameraUp) * fScale);
	fsTex = vec2(1.0f, 1.0f);
	EmitVertex();

	EndPrimitive();
}	