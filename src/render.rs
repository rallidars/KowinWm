use smithay::{
    backend::{
        drm::DrmDeviceFd,
        renderer::{
            element::{
                surface::WaylandSurfaceRenderElement, texture::TextureRenderElement, Element, Id,
                RenderElement,
            },
            gles::{element::PixelShaderElement, GlesFrame, GlesTexture, Uniform},
            glow::{GlowFrame, GlowRenderer},
            multigpu::{self, gbm::GbmGlesBackend, MultiFrame, MultiRenderer},
            utils::{CommitCounter, DamageSet},
            ImportAll, ImportMem, Renderer, RendererSuper, Texture,
        },
    },
    utils::{Buffer, Physical, Rectangle, Scale},
};

pub type GlMultiRenderer<'a> = MultiRenderer<
    'a,
    'a,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
>;
pub type GlMultiFrame<'a, 'frame> = MultiFrame<
    'a,
    'a,
    'a,
    'frame,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
>;

pub type MultiError<'a> = multigpu::Error<
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
    GbmGlesBackend<GlowRenderer, DrmDeviceFd>,
>;
pub enum CustomRenderElements<R>
where
    R: Renderer,
{
    Texture(TextureRenderElement<GlesTexture>),
    Window(WaylandSurfaceRenderElement<R>),
    Shader(PixelShaderElement),
}

impl<R> Element for CustomRenderElements<R>
where
    R: Renderer,
    <R as RendererSuper>::TextureId: 'static,
    R: ImportAll + ImportMem,
{
    fn id(&self) -> &Id {
        match self {
            CustomRenderElements::Texture(elem) => elem.id(),
            CustomRenderElements::Window(elem) => elem.id(),
            CustomRenderElements::Shader(elem) => elem.id(),
        }
    }
    fn src(&self) -> Rectangle<f64, Buffer> {
        match self {
            CustomRenderElements::Texture(elem) => elem.src(),
            CustomRenderElements::Window(elem) => elem.src(),
            CustomRenderElements::Shader(elem) => elem.src(),
        }
    }
    fn geometry(&self, scale: Scale<f64>) -> Rectangle<i32, Physical> {
        match self {
            CustomRenderElements::Texture(elem) => elem.geometry(scale),
            CustomRenderElements::Window(elem) => elem.geometry(scale),
            CustomRenderElements::Shader(elem) => elem.geometry(scale),
        }
    }
    fn current_commit(&self) -> CommitCounter {
        match self {
            CustomRenderElements::Texture(elem) => elem.current_commit(),
            CustomRenderElements::Window(elem) => elem.current_commit(),
            CustomRenderElements::Shader(elem) => elem.current_commit(),
        }
    }
    fn opaque_regions(
        &self,
        scale: Scale<f64>,
    ) -> smithay::backend::renderer::utils::OpaqueRegions<i32, Physical> {
        match self {
            CustomRenderElements::Texture(elem) => elem.opaque_regions(scale),
            CustomRenderElements::Window(elem) => elem.opaque_regions(scale),
            CustomRenderElements::Shader(elem) => elem.opaque_regions(scale),
        }
    }
    fn kind(&self) -> smithay::backend::renderer::element::Kind {
        match self {
            CustomRenderElements::Texture(elem) => elem.kind(),
            CustomRenderElements::Window(elem) => elem.kind(),
            CustomRenderElements::Shader(elem) => elem.kind(),
        }
    }
    fn alpha(&self) -> f32 {
        match self {
            CustomRenderElements::Texture(elem) => elem.alpha(),
            CustomRenderElements::Window(elem) => elem.alpha(),
            CustomRenderElements::Shader(elem) => elem.alpha(),
        }
    }
    fn location(&self, scale: Scale<f64>) -> smithay::utils::Point<i32, Physical> {
        match self {
            CustomRenderElements::Texture(elem) => elem.location(scale),
            CustomRenderElements::Window(elem) => elem.location(scale),
            CustomRenderElements::Shader(elem) => elem.location(scale),
        }
    }
    fn transform(&self) -> smithay::utils::Transform {
        match self {
            CustomRenderElements::Texture(elem) => elem.transform(),
            CustomRenderElements::Window(elem) => elem.transform(),
            CustomRenderElements::Shader(elem) => elem.transform(),
        }
    }
    fn damage_since(
        &self,
        scale: Scale<f64>,
        commit: Option<CommitCounter>,
    ) -> DamageSet<i32, Physical> {
        match self {
            CustomRenderElements::Texture(elem) => elem.damage_since(scale, commit),
            CustomRenderElements::Window(elem) => elem.damage_since(scale, commit),
            CustomRenderElements::Shader(elem) => elem.damage_since(scale, commit),
        }
    }
}

impl<'a> RenderElement<GlMultiRenderer<'a>> for CustomRenderElements<GlMultiRenderer<'a>> {
    fn draw(
        &self,
        frame: &mut <GlMultiRenderer<'a> as RendererSuper>::Frame<'_, '_>,
        src: Rectangle<f64, Buffer>,
        dst: Rectangle<i32, Physical>,
        damage: &[Rectangle<i32, Physical>],
        opaque_regions: &[Rectangle<i32, Physical>],
    ) -> Result<(), <GlMultiRenderer<'a> as RendererSuper>::Error> {
        match self {
            CustomRenderElements::Texture(elem) => RenderElement::<GlowRenderer>::draw(
                elem,
                frame.as_mut(),
                src,
                dst,
                damage,
                opaque_regions,
            )
            .map_err(MultiError::Render),
            CustomRenderElements::Window(elem) => {
                elem.draw(frame, src, dst, damage, opaque_regions)
            }
            CustomRenderElements::Shader(elem) => RenderElement::<GlowRenderer>::draw(
                elem,
                frame.as_mut(),
                src,
                dst,
                damage,
                opaque_regions,
            )
            .map_err(MultiError::Render),
        }
    }

    fn underlying_storage(
        &self,
        renderer: &mut GlMultiRenderer<'a>,
    ) -> Option<smithay::backend::renderer::element::UnderlyingStorage<'_>> {
        match self {
            CustomRenderElements::Texture(elem) => elem.underlying_storage(renderer.as_mut()),
            CustomRenderElements::Window(elem) => elem.underlying_storage(renderer),
            CustomRenderElements::Shader(elem) => elem.underlying_storage(renderer.as_mut()),
        }
    }
}
impl<R> From<TextureRenderElement<GlesTexture>> for CustomRenderElements<R>
where
    R: Renderer,
{
    fn from(value: TextureRenderElement<GlesTexture>) -> Self {
        CustomRenderElements::Texture(value)
    }
}
impl<R> From<PixelShaderElement> for CustomRenderElements<R>
where
    R: Renderer,
{
    fn from(value: PixelShaderElement) -> Self {
        CustomRenderElements::Shader(value)
    }
}

impl<R> From<WaylandSurfaceRenderElement<R>> for CustomRenderElements<R>
where
    R: Renderer,
{
    fn from(value: WaylandSurfaceRenderElement<R>) -> Self {
        CustomRenderElements::Window(value)
    }
}
