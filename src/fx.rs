//! Эффекты (фильтры картинки) и переходы между обоями: шейдеры GLSL.

pub struct Param {
    pub name: &'static str,
    pub min: f32,
    pub max: f32,
    pub def: f32,
}
const fn p(name: &'static str, min: f32, max: f32, def: f32) -> Param {
    Param { name, min, max, def }
}

pub struct FxDef {
    pub id: &'static str,
    pub name: &'static str,
    pub group: &'static str,
    pub desc: &'static str,
    pub code: i32,
    pub params: &'static [Param],
    pub colors: &'static [(&'static str, [f32; 3])],
}

pub const GROUPS: [&str; 6] = ["Цвет", "Коррекция", "Тон", "Стиль", "Искажение", "Кадр"];

pub const FX: &[FxDef] = &[
    FxDef { id: "palette", name: "Палитра", group: "Цвет", desc: "Заменяет каждый цвет ближайшим из палитры.", code: 1, params: &[p("Палитра", 0.0, 9.0, 0.0), p("Сила", 0.0, 1.0, 1.0)], colors: &[] },
    FxDef { id: "duotone", name: "Два тона", group: "Цвет", desc: "Тени красит первым цветом, света — вторым.", code: 2, params: &[p("Сила", 0.0, 1.0, 1.0)], colors: &[("Тени", [0.11, 0.13, 0.25]), ("Света", [0.95, 0.80, 0.62])] },
    FxDef { id: "sepia", name: "Сепия", group: "Цвет", desc: "Тёплый коричневый тон старой фотографии.", code: 3, params: &[p("Сила", 0.0, 1.0, 0.8)], colors: &[] },
    FxDef { id: "hue", name: "Оттенок", group: "Цвет", desc: "Поворачивает все цвета по цветовому кругу.", code: 4, params: &[p("Поворот, °", -180.0, 180.0, 40.0)], colors: &[] },
    FxDef { id: "temperature", name: "Температура", group: "Цвет", desc: "Делает картинку теплее или холоднее.", code: 5, params: &[p("Тепло", -1.0, 1.0, 0.3), p("Зелень/пурпур", -1.0, 1.0, 0.0)], colors: &[] },
    FxDef { id: "tint", name: "Тонирование", group: "Цвет", desc: "Окрашивает картинку одним цветом, сохраняя яркость.", code: 6, params: &[p("Сила", 0.0, 1.0, 0.5)], colors: &[("Цвет", [0.53, 0.75, 0.82])] },
    FxDef { id: "aberration", name: "Хроматическая аберрация", group: "Цвет", desc: "Разводит цветовые каналы к краям кадра.", code: 7, params: &[p("Сила", 0.0, 1.0, 0.3)], colors: &[] },
    FxDef { id: "dim", name: "Затемнение", group: "Коррекция", desc: "Приглушает обои, чтобы окна читались лучше.", code: 30, params: &[p("Сила", 0.0, 1.0, 0.35)], colors: &[] },
    FxDef { id: "brightness", name: "Яркость", group: "Коррекция", desc: "Светлее или темнее.", code: 8, params: &[p("Яркость", -1.0, 1.0, 0.15)], colors: &[] },
    FxDef { id: "contrast", name: "Контраст", group: "Коррекция", desc: "Разница между светлым и тёмным.", code: 9, params: &[p("Контраст", -1.0, 1.0, 0.25)], colors: &[] },
    FxDef { id: "saturation", name: "Насыщенность", group: "Коррекция", desc: "Сочность цветов.", code: 10, params: &[p("Насыщенность", -1.0, 1.0, 0.3)], colors: &[] },
    FxDef { id: "gamma", name: "Гамма", group: "Коррекция", desc: "Яркость средних тонов.", code: 11, params: &[p("Гамма", 0.3, 3.0, 1.3)], colors: &[] },
    FxDef { id: "exposure", name: "Экспозиция", group: "Коррекция", desc: "Общее количество света, в ступенях.", code: 12, params: &[p("Ступени", -2.0, 2.0, 0.5)], colors: &[] },
    FxDef { id: "grayscale", name: "Оттенки серого", group: "Тон", desc: "Убирает цвет.", code: 22, params: &[p("Сила", 0.0, 1.0, 1.0)], colors: &[] },
    FxDef { id: "invert", name: "Негатив", group: "Тон", desc: "Обращает цвета.", code: 23, params: &[p("Сила", 0.0, 1.0, 1.0)], colors: &[] },
    FxDef { id: "threshold", name: "Порог", group: "Тон", desc: "Оставляет два цвета: темнее и светлее порога.", code: 24, params: &[p("Порог", 0.0, 1.0, 0.5), p("Мягкость", 0.0, 0.5, 0.05)], colors: &[("Тёмное", [0.07, 0.08, 0.10]), ("Светлое", [0.93, 0.94, 0.96])] },
    FxDef { id: "posterize", name: "Постеризация", group: "Тон", desc: "Уменьшает число оттенков.", code: 17, params: &[p("Уровней", 2.0, 16.0, 5.0)], colors: &[] },
    FxDef { id: "pixelate", name: "Пикселизация", group: "Стиль", desc: "Крупные квадратные пиксели.", code: 16, params: &[p("Размер", 2.0, 64.0, 10.0)], colors: &[] },
    FxDef { id: "halftone", name: "Полутон", group: "Стиль", desc: "Газетный растр из точек.", code: 21, params: &[p("Размер точки", 3.0, 24.0, 7.0)], colors: &[] },
    FxDef { id: "grain", name: "Зерно", group: "Стиль", desc: "Плёночный шум.", code: 18, params: &[p("Сила", 0.0, 1.0, 0.25)], colors: &[] },
    FxDef { id: "vignette", name: "Виньетка", group: "Стиль", desc: "Затемняет края кадра.", code: 19, params: &[p("Сила", 0.0, 1.0, 0.5), p("Радиус", 0.2, 1.2, 0.75)], colors: &[] },
    FxDef { id: "scanlines", name: "Строки развёртки", group: "Стиль", desc: "Полосы старого кинескопа.", code: 20, params: &[p("Сила", 0.0, 1.0, 0.3), p("Плотность", 100.0, 800.0, 360.0)], colors: &[] },
    FxDef { id: "blur", name: "Размытие", group: "Искажение", desc: "Мягкое размытие по Гауссу.", code: 25, params: &[p("Радиус", 1.0, 40.0, 10.0)], colors: &[] },
    FxDef { id: "swirl", name: "Вихрь", group: "Искажение", desc: "Закручивает картинку вокруг центра.", code: 27, params: &[p("Угол", -3.0, 3.0, 1.0), p("Радиус", 0.2, 1.0, 0.6)], colors: &[] },
    FxDef { id: "wave", name: "Волна", group: "Искажение", desc: "Синусоидальная рябь.", code: 28, params: &[p("Амплитуда", 0.0, 0.05, 0.012), p("Частота", 2.0, 40.0, 12.0)], colors: &[] },
    FxDef { id: "fisheye", name: "Рыбий глаз", group: "Искажение", desc: "Выпуклая или вогнутая линза.", code: 29, params: &[p("Сила", -1.0, 1.0, 0.4)], colors: &[] },
    FxDef { id: "flip-h", name: "Отразить по горизонтали", group: "Кадр", desc: "Зеркало слева направо.", code: 13, params: &[], colors: &[] },
    FxDef { id: "flip-v", name: "Отразить по вертикали", group: "Кадр", desc: "Зеркало сверху вниз.", code: 14, params: &[], colors: &[] },
    FxDef { id: "zoom", name: "Приближение", group: "Кадр", desc: "Увеличивает и сдвигает кадр.", code: 15, params: &[p("Масштаб", 1.0, 3.0, 1.3), p("Сдвиг X", -1.0, 1.0, 0.0), p("Сдвиг Y", -1.0, 1.0, 0.0)], colors: &[] },
];

pub fn def(id: &str) -> Option<&'static FxDef> {
    FX.iter().find(|f| f.id == id)
}

pub fn instance(d: &FxDef) -> crate::library::Fx {
    let mut f = crate::library::Fx { id: d.id.into(), ..Default::default() };
    for (i, p) in d.params.iter().enumerate().take(4) {
        f.p[i] = p.def;
    }
    for (i, c) in d.colors.iter().enumerate().take(2) {
        f.c[i] = c.1;
    }
    f
}

pub const PALETTES: &[(&str, &[u32])] = &[
    ("Catppuccin", &[0x1e1e2e, 0x313244, 0x585b70, 0xcdd6f4, 0xf5e0dc, 0xf38ba8, 0xfab387, 0xf9e2af, 0xa6e3a1, 0x94e2d5, 0x89dceb, 0x89b4fa, 0xb4befe, 0xcba6f7, 0xf5c2e7]),
    ("Nord", &[0x2e3440, 0x3b4252, 0x434c5e, 0x4c566a, 0xd8dee9, 0xe5e9f0, 0xeceff4, 0x8fbcbb, 0x88c0d0, 0x81a1c1, 0x5e81ac, 0xbf616a, 0xd08770, 0xebcb8b, 0xa3be8c, 0xb48ead]),
    ("Everforest", &[0x2d353b, 0x343f44, 0x475258, 0x859289, 0xd3c6aa, 0xe67e80, 0xe69875, 0xdbbc7f, 0xa7c080, 0x83c092, 0x7fbbb3, 0xd699b6]),
    ("Solarized", &[0x002b36, 0x073642, 0x586e75, 0x657b83, 0x839496, 0x93a1a1, 0xeee8d5, 0xfdf6e3, 0xb58900, 0xcb4b16, 0xdc322f, 0xd33682, 0x6c71c4, 0x268bd2, 0x2aa198, 0x859900]),
    ("Gruvbox", &[0x282828, 0x3c3836, 0x504945, 0x928374, 0xebdbb2, 0xfbf1c7, 0xcc241d, 0xd65d0e, 0xd79921, 0x98971a, 0x689d6a, 0x458588, 0xb16286, 0xfb4934, 0xfabd2f, 0x83a598]),
    ("Dracula", &[0x282a36, 0x44475a, 0x6272a4, 0xf8f8f2, 0x8be9fd, 0x50fa7b, 0xffb86c, 0xff79c6, 0xbd93f9, 0xff5555, 0xf1fa8c]),
    ("Tokyo Night", &[0x1a1b26, 0x24283b, 0x414868, 0x565f89, 0xa9b1d6, 0xc0caf5, 0xf7768e, 0xff9e64, 0xe0af68, 0x9ece6a, 0x73daca, 0x7dcfff, 0x7aa2f7, 0xbb9af7]),
    ("One Dark", &[0x282c34, 0x3e4451, 0x5c6370, 0xabb2bf, 0xe06c75, 0xd19a66, 0xe5c07b, 0x98c379, 0x56b6c2, 0x61afef, 0xc678dd]),
    ("Rosé Pine", &[0x191724, 0x1f1d2e, 0x26233a, 0x6e6a86, 0x908caa, 0xe0def4, 0xeb6f92, 0xf6c177, 0xebbcba, 0x31748f, 0x9ccfd8, 0xc4a7e7]),
    ("Чёрно-белая", &[0x000000, 0x333333, 0x666666, 0x999999, 0xcccccc, 0xffffff]),
];

pub const TRANSITIONS: &[(&str, &str)] = &[
    ("fade", "Растворение"),
    ("wipe", "Шторка"),
    ("center", "Из центра"),
    ("grow", "Круг наружу"),
    ("outer", "Круг внутрь"),
    ("slide", "Сдвиг"),
    ("crosswarp", "Перетекание"),
    ("zoom", "Наезд"),
    ("pixelize", "Пиксели"),
    ("squares", "Квадраты"),
    ("dots", "Горошек"),
    ("perlin", "Дымка"),
    ("ripple", "Рябь"),
    ("swirl", "Вихрь"),
    ("blinds", "Жалюзи"),
    ("glitch", "Помехи"),
    ("morph", "Морфинг"),
    ("wave", "Волна"),
];

pub fn transition_index(name: &str, seed: u32) -> i32 {
    match TRANSITIONS.iter().position(|t| t.0 == name) {
        Some(i) => i as i32,
        None if name == "none" => -1,
        None => (seed % TRANSITIONS.len() as u32) as i32,
    }
}

pub fn transition_label(name: &str) -> &'static str {
    match name {
        "random" => crate::i18n::tr("Случайный"),
        "none" => crate::i18n::tr("Без перехода"),
        _ => TRANSITIONS.iter().find(|t| t.0 == name).map(|t| crate::i18n::tr(t.1)).unwrap_or(crate::i18n::tr("Случайный")),
    }
}

pub const FULL_VS: &str = r#"
layout(location=0) in vec2 a_pos; uniform float u_flip; out vec2 v_uv;
void main(){ v_uv = vec2(a_pos.x*0.5+0.5, 0.5-a_pos.y*0.5); gl_Position = vec4(a_pos.x, a_pos.y*u_flip, 0.0, 1.0); }
"#;

pub const WALL_FS: &str = r#"
uniform sampler2D u_a; uniform sampler2D u_b;
uniform vec4 u_fa; uniform vec4 u_fb; uniform float u_has_a; uniform float u_has_b;
uniform float u_t; uniform int u_type; uniform float u_ratio; uniform float u_seed;
in vec2 v_uv; out vec4 o;
vec4 smp(sampler2D s, vec4 f, float has, vec2 uv){
  uv = clamp(uv,0.0,1.0);
  vec2 t = (uv-0.5)*f.xy+0.5;
  if(has<0.5 || t.x<0.0 || t.y<0.0 || t.x>1.0 || t.y>1.0) return vec4(0.0,0.0,0.0,1.0);
  return vec4(texture(s,t).rgb,1.0);
}
vec4 A(vec2 uv){ return smp(u_a,u_fa,u_has_a,uv); }
vec4 B(vec2 uv){ return smp(u_b,u_fb,u_has_b,uv); }
float rand(vec2 co){ return fract(sin(dot(co, vec2(12.9898,78.233))) * 43758.5453); }
float noise(vec2 p){ vec2 i=floor(p), f=fract(p); f=f*f*(3.0-2.0*f);
  return mix(mix(rand(i),rand(i+vec2(1.0,0.0)),f.x), mix(rand(i+vec2(0.0,1.0)),rand(i+vec2(1.0,1.0)),f.x), f.y); }
float fbm(vec2 p){ return 0.5*noise(p)+0.25*noise(p*2.03)+0.125*noise(p*4.01)+0.125*noise(p*8.07); }
void main(){
  vec2 uv = v_uv; float p = u_t; vec4 c;
  if(u_type<=0){ c = mix(A(uv),B(uv),p); }
  else if(u_type==1){ float pos = mix(-0.25,1.25,p); float m = 1.0-smoothstep(pos-0.07,pos+0.07, uv.x-(uv.y-0.5)*0.2); c = mix(A(uv),B(uv),m); }
  else if(u_type==2){ float d = abs(uv.x-0.5+(uv.y-0.5)*0.18); float r = p*0.64; float m = 1.0-smoothstep(r-0.015,r+0.015,d); c = mix(A(uv),B(uv),m); }
  else if(u_type==3){ vec2 q=(uv-0.5)*vec2(u_ratio,1.0); float r = p*length(vec2(u_ratio,1.0))*0.56; float m = 1.0-smoothstep(r-0.03,r+0.03,length(q)); c = mix(A(uv),B(uv),m); }
  else if(u_type==4){ vec2 q=(uv-0.5)*vec2(u_ratio,1.0); float r = (1.0-p)*length(vec2(u_ratio,1.0))*0.56; float m = smoothstep(r-0.03,r+0.03,length(q)); c = mix(A(uv),B(uv),m); }
  else if(u_type==5){ float x = uv.x+p; c = x<1.0 ? A(vec2(x,uv.y)) : B(vec2(x-1.0,uv.y)); }
  else if(u_type==6){ float x = smoothstep(0.0,1.0,p*2.0+uv.x-1.0); c = mix(A((uv-0.5)*(1.0-x)+0.5), B((uv-0.5)*x+0.5), x); }
  else if(u_type==7){ float s = 0.45*sin(p*3.14159265); float m = smoothstep(0.25,0.75,p); vec4 acc = vec4(0.0);
    for(int i=0;i<12;i++){ float f = float(i)/11.0; vec2 q = 0.5+(uv-0.5)*(1.0-s*f); acc += mix(A(q),B(q),m); } c = acc/12.0; }
  else if(u_type==8){ float d = min(p,1.0-p); vec2 n = vec2(u_ratio,1.0)*mix(240.0,14.0,sqrt(d*2.0)); vec2 q = d<0.01 ? uv : (floor(uv*n)+0.5)/n; c = mix(A(q),B(q),smoothstep(0.35,0.65,p)); }
  else if(u_type==9){ float r = rand(floor(vec2(16.0,9.0)*uv)+u_seed); float m = smoothstep(0.0,-0.3,r-p*1.3); c = mix(A(uv),B(uv),m); }
  else if(u_type==10){ vec2 g = vec2(22.0*u_ratio,22.0); float d = distance(fract(uv*g),vec2(0.5)); float m = step(d, p*1.6/max(distance(uv*vec2(u_ratio,1.0),vec2(0.0)),0.001)); m = max(m, step(0.995,p)); c = mix(A(uv),B(uv),m); }
  else if(u_type==11){ float n = fbm(uv*vec2(u_ratio,1.0)*4.0+u_seed*17.0); float pp = mix(-0.2,1.2,p); float m = 1.0-smoothstep(pp-0.12,pp+0.12,n); c = mix(A(uv),B(uv),m); }
  else if(u_type==12){ vec2 dir = uv-0.5; float dist = length(dir*vec2(u_ratio,1.0)); vec2 off = dir*(sin(p*dist*40.0-p*20.0)+0.5)/30.0*(1.0-p); c = mix(A(uv+off),B(uv+off*0.3),smoothstep(0.2,1.0,p)); }
  else if(u_type==13){ vec2 q = uv-0.5; float d = length(q); float pc = max(1.0-d,0.0); float a = p<=0.5 ? p*2.0 : (1.0-p)*2.0;
    float th = pc*pc*a*6.2831853*1.5; float s=sin(th), k=cos(th); q = vec2(q.x*k-q.y*s, q.x*s+q.y*k)+0.5; c = mix(A(q),B(q),p); }
  else if(u_type==14){ float pr = smoothstep(-0.5,0.0,uv.x-p*1.5); float s = step(pr,fract(12.0*uv.x)); c = mix(A(uv),B(uv),s); }
  else if(u_type==15){ float amt = sin(p*3.14159265); float b = floor(uv.y*28.0); float r = rand(vec2(b,floor(p*14.0)+u_seed));
    float sh = (r-0.5)*0.3*amt*step(0.55,rand(vec2(b*1.7,floor(p*9.0)))); vec2 q = vec2(fract(uv.x+sh+1.0),uv.y); float m = smoothstep(0.4,0.6,p);
    c = mix(A(q),B(q),m); vec2 q2 = q+vec2(0.012*amt,0.0); c.r = mix(A(q2).r,B(q2).r,m); }
  else if(u_type==16){ vec4 ca=A(uv), cb=B(uv); vec2 oa = ((ca.rg+ca.b)*0.5)*2.0-1.0; vec2 ob = ((cb.rg+cb.b)*0.5)*2.0-1.0; vec2 oc = mix(oa,ob,0.5)*0.12; c = mix(A(uv+oc*p),B(uv-oc*(1.0-p)),p); }
  else { float edge = uv.x+sin(uv.y*11.0+p*5.0)*0.035+(uv.y-0.5)*0.15; float pp = mix(-0.2,1.2,p); float m = 1.0-smoothstep(pp-0.04,pp+0.04,edge); c = mix(A(uv),B(uv),m); }
  o = vec4(c.rgb,1.0);
}
"#;

pub const FX_FS: &str = r#"
uniform sampler2D u_tex; uniform int u_fx; uniform vec4 u_p; uniform vec3 u_c0; uniform vec3 u_c1;
uniform vec2 u_px; uniform vec3 u_pal[16]; uniform int u_paln;
in vec2 v_uv; out vec4 o;
float luma(vec3 c){ return dot(c,vec3(0.2126,0.7152,0.0722)); }
float rand(vec2 co){ return fract(sin(dot(co, vec2(12.9898,78.233))) * 43758.5453); }
vec3 T(vec2 uv){ return texture(u_tex,clamp(uv,0.0,1.0)).rgb; }
void main(){
  vec2 uv = v_uv; vec3 c;
  if(u_fx==13) uv.x = 1.0-uv.x;
  else if(u_fx==14) uv.y = 1.0-uv.y;
  else if(u_fx==15){ float z = max(u_p.x,1.0); vec2 room = vec2(0.5-0.5/z); uv = (uv-0.5)/z+0.5+room*u_p.yz; }
  else if(u_fx==16){ vec2 s = u_px*max(u_p.x,1.0); uv = (floor(uv/s)+0.5)*s; }
  else if(u_fx==27){ vec2 q = uv-0.5; q.x *= u_px.y/u_px.x; float d = length(q); float r = max(u_p.y,0.01)*0.5*(u_px.y/u_px.x);
    if(d<r){ float k = (r-d)/r; float th = k*k*u_p.x*3.14159265; float s=sin(th), cs=cos(th); q = vec2(q.x*cs-q.y*s,q.x*s+q.y*cs); }
    q.x /= u_px.y/u_px.x; uv = q+0.5; }
  else if(u_fx==28){ uv += vec2(sin(uv.y*u_p.y*6.2831853), cos(uv.x*u_p.y*6.2831853))*u_p.x; }
  else if(u_fx==29){ vec2 q = uv*2.0-1.0; float d = length(q); float k = 1.0+u_p.x*(d*d-1.0)*0.6; uv = q*k*0.5+0.5; }
  if(u_fx==25 || u_fx==26){
    vec2 dir = (u_fx==25 ? vec2(u_px.x,0.0) : vec2(0.0,u_px.y))*max(u_p.x,0.0)/8.0; vec3 acc = vec3(0.0); float tot = 0.0;
    for(int i=-12;i<=12;i++){ float w = exp(-float(i*i)/40.0); acc += T(uv+dir*float(i)*(8.0/12.0)*1.5)*w; tot += w; }
    c = acc/tot;
  } else if(u_fx==7){ vec2 d = (uv-0.5)*u_p.x*0.04; c = vec3(T(uv+d).r, T(uv).g, T(uv-d).b); }
  else if(u_fx==21){ float s = max(u_p.x,2.0); vec2 cell = u_px*s; vec2 g = (floor(uv/cell)+0.5)*cell; vec3 t = T(g);
    float d = length((uv-g)/cell); float r = sqrt(clamp(luma(t),0.0,1.0))*0.72; c = mix(vec3(0.04),t*1.15,1.0-smoothstep(r-0.08,r+0.08,d)); }
  else c = T(uv);
  float l = luma(c);
  if(u_fx==1){ vec3 best = c; float bd = 1e9; for(int i=0;i<16;i++){ if(i>=u_paln) break; vec3 d = c-u_pal[i]; float dd = dot(d,d); if(dd<bd){ bd=dd; best=u_pal[i]; } } c = mix(c,best,u_p.y); }
  else if(u_fx==2) c = mix(c, mix(u_c0,u_c1,l), u_p.x);
  else if(u_fx==3) c = mix(c, vec3(dot(c,vec3(0.393,0.769,0.189)),dot(c,vec3(0.349,0.686,0.168)),dot(c,vec3(0.272,0.534,0.131))), u_p.x);
  else if(u_fx==4){ float a = radians(u_p.x); float s=sin(a), k=cos(a); vec3 w = vec3(0.57735);
    c = c*k + cross(w,c)*s + w*dot(w,c)*(1.0-k); }
  else if(u_fx==5){ c.r += u_p.x*0.12; c.b -= u_p.x*0.12; c.g += u_p.y*0.08; }
  else if(u_fx==6) c = mix(c, u_c0*l*1.5, u_p.x);
  else if(u_fx==8) c += u_p.x*0.5;
  else if(u_fx==9) c = (c-0.5)*(1.0+u_p.x*1.5)+0.5;
  else if(u_fx==10) c = mix(vec3(l),c,1.0+u_p.x*1.5);
  else if(u_fx==11) c = pow(max(c,0.0),vec3(1.0/max(u_p.x,0.05)));
  else if(u_fx==12) c *= pow(2.0,u_p.x);
  else if(u_fx==17){ float n = max(u_p.x,2.0); c = floor(c*n)/(n-1.0); }
  else if(u_fx==18) c += (rand(uv*vec2(913.0,517.0))-0.5)*u_p.x*0.35;
  else if(u_fx==19){ vec2 q = (uv-0.5)*vec2(1.0,u_px.x/u_px.y); float d = length(q)*1.6; c *= 1.0-u_p.x*smoothstep(u_p.y*0.6,u_p.y*0.6+0.6,d); }
  else if(u_fx==20) c *= 1.0-u_p.x*(0.5+0.5*sin(uv.y*u_p.y*6.2831853));
  else if(u_fx==22) c = mix(c,vec3(l),u_p.x);
  else if(u_fx==23) c = mix(c,1.0-c,u_p.x);
  else if(u_fx==24) c = mix(u_c0,u_c1,smoothstep(u_p.x-u_p.y-0.001,u_p.x+u_p.y+0.001,l));
  else if(u_fx==30) c *= 1.0-u_p.x;
  o = vec4(clamp(c,0.0,1.0),1.0);
}
"#;
