// Native background/decor only. No screen, scene, icon or glyph texture is read.
struct UiVertexOutput {
    @location(0) uv:vec2<f32>, @location(1) border_widths:vec4<f32>,
    @location(2) border_radius:vec4<f32>, @location(3) @interpolate(flat) size:vec2<f32>,
    @builtin(position) position:vec4<f32>,
};
struct OpticalUniforms {
    tint:vec4<f32>, phase:vec4<f32>, source:vec4<f32>,
    signal:vec4<f32>, detail:vec4<f32>, motion:vec4<f32>,
};
@group(1) @binding(0) var<uniform> material:OpticalUniforms;
fn hash(p:vec2<f32>)->f32 {
    let q=vec2<u32>(max(floor(p),vec2<f32>(0.)));
    var n=q.x*1973u+q.y*9277u+89173u;
    n=(n^(n>>13u))*1274126177u;n=n^(n>>16u);
    return f32(n&65535u)/65535.;
}
fn line(x:f32,width:f32)->f32 {return 1.-smoothstep(width,width+1.,abs(x));}
@fragment fn fragment(in:UiVertexOutput)->@location(0) vec4<f32> {
    let tint=material.tint;let phase=material.phase;let source=material.source;
    let signal=material.signal;let detail=material.detail;let motion=material.motion;
    if detail.z<0.5 {return vec4<f32>(0.);}
    // Zero effects uses only the flat optical foundation. Pixel/data/CRT
    // MaterialNodes are removed entirely by the native lifecycle.
    if motion.x<=0. {return vec4<f32>(tint.rgb,tint.a*phase.y);}
    let p=in.uv*in.size;let edge=min(p,in.size-p);let d=min(edge.x,edge.y);
    let rim=1.-smoothstep(0.5,1.5,d);let rim_zone=1.-smoothstep(3.,9.,d);
    let corners=(1.-smoothstep(9.,14.,edge.x))*(1.-smoothstep(9.,14.,edge.y));
    let flash=signal.z;let selected=signal.x;let hover=signal.y;
    let amber=vec3<f32>(1.,0.61,0.12);let cyan=vec3<f32>(0.30,0.79,0.97);
    // A sync fault shifts only a narrow decorative rim band. Text never enters here.
    let sync_band=1.-smoothstep(2.,5.,abs(p.y-(0.15+0.70*fract(detail.w*0.618))*in.size.y));
    let sync_mask=sync_band*select(rim_zone,1.,phase.z>2.5);
    let q=p+vec2<f32>(detail.x*sync_mask,0.);
    let noise=(hash(q+vec2<f32>(detail.w))-0.5);
    let event_noise=(hash(q+floor(phase.x*30.)*vec2<f32>(11.,7.))-0.5)*detail.y;
    var rgb=select(cyan,amber,phase.z>2.5);var alpha=0.;var reveal=1.;var foundation=0.;
    if phase.z<1.5 {
        // Pixel phosphor: ordered raster, stepped contour, granular data cells.
        let raster=select(0.,1.,fract(q.y/4.)<0.24);
        let cell=floor(q/3.);let dot=select(0.,1.,fract(q.x/3.)<0.35 && fract(q.y/3.)<0.35);
        let stipple=select(0.,1.,hash(cell+vec2<f32>(detail.w))>0.76);
        let hatch=select(0.,1.,fract((q.x+q.y)/12.)<0.15);
        let bars=select(0.,1.,fract(q.x/7.)<0.30)*select(0.,1.,edge.y<3.);
        let motif=select(hatch*0.009,dot*stipple*0.026,phase.z>0.5);
        let stepped=select(0.,1.,edge.x<3. || edge.y<3.)*select(0.45,1.,fract((q.x+q.y)/8.)<0.65);
        let rail=line(p.x-3.,1.)*selected;
        alpha=motif+raster*0.007+noise*0.006+stepped*(0.035+selected*0.20+hover*0.09)
            +bars*(0.025+selected*0.10)+rail*0.42+corners*(0.04+selected*0.20)
            +exp(-d*0.35)*(0.015+selected*0.055);
        // Different selected categories carry a different small, stable cell cadence.
        alpha+=dot*select(0.,1.,fract((cell.x+cell.y+phase.w)/5.)<0.21)*selected*0.024;
        alpha+=flash*(rim_zone*0.16+corners*0.25);
        // Filters keep the same pixel vocabulary, with quiet inactive rails.
        // Selection owns the local amber phosphor instead of a second cyan box.
        if phase.w>=8. {alpha*=clamp(0.18+selected*0.82+hover*0.45,0.,1.);}
        rgb=mix(cyan,amber,selected);
    } else if phase.z<2.5 {
        // Optical: fine interference, lens ticks and a source-relative aperture.
        let scan=select(0.,1.,fract(q.y/4.)<0.18);
        let moire=select(0.,1.,fract(q.x/23.+q.y/31.)<0.035);
        let ticks=select(0.,1.,fract(q.x/13.)<0.09)*select(0.,1.,edge.y<4.);
        let lens=exp(-abs(length((p-in.size*0.5)/max(in.size,vec2<f32>(1.)))-0.49)*100.);
        // Context recipe chips share the projection's source but keep only
        // corner contours. No extra pass, moving text or opaque card wall.
        let chip=phase.w>0.5;
        let contour=select(rim,rim*corners,chip);
        let glass=tint.a*(0.93-scan*0.10);foundation=glass;
        alpha=glass+contour*(0.13+corners*0.24)+exp(-d*0.28)*0.022+scan*0.006+moire*0.012
            +ticks*select(0.09,0.035,chip)+lens*select(0.014,0.,chip)+noise*0.008+flash*rim_zone*0.12;
        rgb=mix(tint.rgb,cyan,clamp((alpha-glass)*2.5,0.,0.8));
        if source.w>0.5 {
            let origin=source.xy*in.size;
            let near=length(origin-clamp(origin,vec2<f32>(0.),in.size));
            let far=max(max(length(origin),length(origin-in.size)),max(length(origin-vec2<f32>(in.size.x,0.)),length(origin-vec2<f32>(0.,in.size.y))));
            let radius=mix(near-12.,far+24.,phase.y);let distance=length(p-origin);
            reveal=1.-smoothstep(radius-12.,radius,distance);
            alpha+=exp(-abs(distance-radius+6.)*0.2)*(1.-phase.y)*0.16;
        } else {reveal=phase.y;}
    } else {
        // A short local CRT insert. Exactly transparent after its triggering event.
        let raster=select(0.,1.,fract(q.y/3.)<0.30);
        let sweep=line(q.y-in.size.y*(1.-flash),1.2);
        let blocks=select(0.,1.,hash(floor(q/vec2<f32>(12.,3.))+vec2<f32>(detail.w))>0.78);
        alpha=flash*(raster*0.08+sweep*0.40+blocks*0.055+rim*0.2)+event_noise*0.06;
    }
    // At most 1.25px split, only on procedural edge glints and only during an event.
    let split=signal.w;
    let red_edge=line(d-split,0.7)*rim_zone;
    let blue_edge=line(d+split-2.,0.7)*rim_zone;
    let chroma=clamp(split*0.35,0.,0.4);
    rgb=mix(rgb,mix(amber,cyan,blue_edge),chroma*max(red_edge,blue_edge));
    alpha+=event_noise*rim_zone*0.012+split*rim_zone*0.025;
    alpha=foundation+(alpha-foundation)*motion.x;
    return vec4<f32>(rgb,clamp(alpha,0.,0.88)*reveal);
}
