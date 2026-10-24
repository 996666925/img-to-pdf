//! 主界面：拖入图片 → 调整顺序与页面参数 → 导出 PDF。

use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui_kit::component::button::{Button, ButtonVariants};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::component::separator::Separator;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{
    ActiveTheme, Disableable, Icon, IconName, Sizable, StyledExt, Theme, ThemeMode, TitleBar,
    h_flex, v_flex,
};
use gpui_kit::*;

use crate::pdf::{self, FitMode, Margin, Orientation, PageSize, PdfOptions};
use crate::thumbs;

/// 支持的图片扩展名。
const SUPPORTED_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "jpe", "jfif", "webp", "gif", "bmp", "tif", "tiff", "ico", "tga", "qoi",
    "pnm", "ppm", "pgm", "pbm", "dds", "hdr",
];

const FORMAT_LABELS: [&str; 7] = ["PNG", "JPG", "WebP", "GIF", "BMP", "TIFF", "ICO"];

pub struct MainView {
    items: Vec<ImageItem>,
    options: PdfOptions,
    status: Status,
    busy: bool,
    last_dir: Option<PathBuf>,
    next_id: u64,
    /// 启动时通过命令行带入、等待加入队列的图片。
    pending: Vec<PathBuf>,
}

struct ImageItem {
    id: u64,
    path: PathBuf,
    name: SharedString,
    bytes: u64,
    src_dims: Option<(u32, u32)>,
    thumb: Option<Arc<RenderImage>>,
    failed: bool,
}

enum Status {
    Idle,
    Working,
    Done {
        file: String,
        pages: usize,
        skipped: usize,
        dir: PathBuf,
    },
    Error(SharedString),
}

impl MainView {
    pub fn new(initial: Vec<PathBuf>, _cx: &mut Context<Self>) -> Self {
        Self {
            items: Vec::new(),
            options: PdfOptions::default(),
            status: Status::Idle,
            busy: false,
            last_dir: None,
            next_id: 0,
            pending: initial,
        }
    }

    fn add_files(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let files = collect_files(&paths);
        let mut added = 0usize;

        for path in files {
            if self.items.iter().any(|item| item.path == path) {
                continue;
            }

            let id = self.next_id;
            self.next_id += 1;
            let bytes = std::fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
            let name: SharedString = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default()
                .into();
            self.items.push(ImageItem {
                id,
                path: path.clone(),
                name,
                bytes,
                src_dims: None,
                thumb: None,
                failed: false,
            });
            added += 1;

            let decode_path = path.clone();
            let job = cx
                .background_executor()
                .spawn(async move { thumbs::decode(&decode_path, 420) });
            cx.spawn(async move |this, cx| {
                let decoded = job.await;
                this.update(cx, |this, cx| {
                    let Some(item) = this.items.iter_mut().find(|item| item.id == id) else {
                        return;
                    };
                    match decoded {
                        Ok(decoded) => {
                            item.src_dims = Some((decoded.src_width, decoded.src_height));
                            item.thumb = decoded.into_render_image();
                            item.failed = item.thumb.is_none();
                        }
                        Err(_) => item.failed = true,
                    }
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }

        if added > 0 {
            if matches!(self.status, Status::Error(_)) {
                self.status = Status::Idle;
            }
            cx.notify();
        }
    }

    fn pick_files(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let receiver = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: true,
            multiple: true,
            prompt: Some("选择图片或文件夹".into()),
        });

        cx.spawn_in(window, async move |this, cx| {
            if let Ok(Ok(Some(paths))) = receiver.await {
                this.update(cx, |this, cx| this.add_files(paths, cx)).ok();
            }
        })
        .detach();
    }

    fn export(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.items.is_empty() || self.busy {
            return;
        }

        let paths: Vec<PathBuf> = self.items.iter().map(|item| item.path.clone()).collect();
        let total = paths.len();
        let options = self.options;
        let dir = self.last_dir.clone().unwrap_or_else(default_dir);
        let suggested = suggested_name(&self.items);
        let receiver = cx.prompt_for_new_path(&dir, Some(&suggested));

        self.busy = true;
        self.status = Status::Working;
        cx.notify();

        cx.spawn_in(window, async move |this, cx| {
            let target = match receiver.await {
                Ok(Ok(Some(path))) => path,
                _ => {
                    this.update(cx, |this, cx| {
                        this.busy = false;
                        this.status = Status::Idle;
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            };

            // 用户没写扩展名时补上 .pdf。
            let target = if target.extension().is_none() {
                target.with_extension("pdf")
            } else {
                target
            };

            let dir = target
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(default_dir);
            let file = target
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();

            // 生成与写入都在后台线程完成，界面保持流畅。
            let job = cx.background_executor().spawn(async move {
                let output = pdf::build(&paths, &options)?;
                std::fs::write(&target, &output.data)
                    .map_err(|err| format!("写入失败：{err}"))?;
                Ok::<pdf::PdfOutput, String>(output)
            });
            let result = job.await;

            this.update(cx, |this, cx| {
                this.busy = false;
                match result {
                    Ok(output) => {
                        this.last_dir = Some(dir.clone());
                        this.status = Status::Done {
                            file,
                            pages: total,
                            skipped: output.skipped.len(),
                            dir,
                        };
                    }
                    Err(err) => {
                        this.status = Status::Error(err.into());
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn move_item(&mut self, index: usize, delta: isize, cx: &mut Context<Self>) {
        let target = index as isize + delta;
        if target < 0 || target as usize >= self.items.len() {
            return;
        }
        self.items.swap(index, target as usize);
        cx.notify();
    }

    fn remove_item(&mut self, index: usize, cx: &mut Context<Self>) {
        if index < self.items.len() {
            self.items.remove(index);
            cx.notify();
        }
    }

    fn clear(&mut self, cx: &mut Context<Self>) {
        self.items.clear();
        self.status = Status::Idle;
        cx.notify();
    }

    fn sort_by_name(&mut self, cx: &mut Context<Self>) {
        self.items
            .sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
        cx.notify();
    }

    // ---------------------------------------------------------------- 渲染

    fn render_title_bar(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (primary, primary_fg) = (theme.primary, theme.primary_foreground);
        let (muted_fg, border) = (theme.muted_foreground, theme.border);
        let is_dark = theme.is_dark();

        TitleBar::new()
            .child(
                h_flex()
                    .flex_1()
                    .h_full()
                    .items_center()
                    .justify_between()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .size(px(22.))
                                    .rounded_md()
                                    .bg(primary)
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        Icon::new(IconName::FileText)
                                            .with_size(px(13.))
                                            .text_color(primary_fg),
                                    ),
                            )
                            .child(div().text_sm().font_semibold().child("图片转 PDF"))
                            .child(div().w(px(1.)).h(px(13.)).bg(border))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child("拖入图片 · 一键合并为 PDF 文档"),
                            ),
                    )
                    .child(
                        h_flex()
                            .items_center()
                            .gap_1()
                            .pr_2()
                            .child(
                                Button::new("theme")
                                    .ghost()
                                    .xsmall()
                                    .icon(if is_dark {
                                        IconName::Sun
                                    } else {
                                        IconName::Moon
                                    })
                                    .tooltip(if is_dark { "切换到浅色" } else { "切换到深色" })
                                    .on_click(|_: &ClickEvent, window: &mut Window, cx: &mut App| {
                                        toggle_theme(window, cx);
                                    }),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_main(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let surface = theme.background;

        let has_items = !self.items.is_empty();
        let toolbar = self.render_toolbar(window, cx);
        let body = if has_items {
            self.render_grid(cx)
        } else {
            self.render_empty(cx)
        };

        let mut container = v_flex()
            .id("drop-area")
            .flex_1()
            .min_w_0()
            .h_full()
            .p_5()
            .gap_4()
            .bg(surface);

        if has_items {
            container = container.child(toolbar);
        }

        container.child(body).into_any_element()
    }

    fn render_toolbar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let muted_fg = theme.muted_foreground;
        let total: u64 = self.items.iter().map(|item| item.bytes).sum();
        let count = self.items.len();

        h_flex()
            .flex_shrink_0()
            .items_center()
            .justify_between()
            .child(
                v_flex()
                    .gap_0p5()
                    .child(div().text_sm().font_semibold().child(format!("{count} 张图片")))
                    .child(
                        div()
                            .text_xs()
                            .text_color(muted_fg)
                            .child(format!("共 {} · 按列表顺序每张生成一页", human_size(total))),
                    ),
            )
            .child(
                h_flex()
                    .items_center()
                    .gap_2()
                    .child(
                        Button::new("sort")
                            .ghost()
                            .small()
                            .label("按名称排序")
                            .icon(IconName::SortAscending)
                            .tooltip("按文件名重新排序")
                            .on_click(cx.listener(|this, _, _, cx| this.sort_by_name(cx))),
                    )
                    .child(
                        Button::new("add")
                            .secondary()
                            .small()
                            .label("添加图片")
                            .icon(IconName::Plus)
                            .on_click(cx.listener(|this, _, window, cx| this.pick_files(window, cx))),
                    )
                    .child(
                        Button::new("clear")
                            .ghost()
                            .small()
                            .label("清空")
                            .icon(IconName::Delete)
                            .disabled(self.busy)
                            .on_click(cx.listener(|this, _, _, cx| this.clear(cx))),
                    ),
            )
            .into_any_element()
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, muted_fg) = (theme.border, theme.muted_foreground);
        let (accent, soft, chip_bg) = (theme.primary, theme.primary.opacity(0.1), theme.muted);

        v_flex()
            .flex_1()
            .h_full()
            .items_center()
            .justify_center()
            .gap_6()
            .p_8()
            .rounded_2xl()
            .border_2()
            .border_dashed()
            .border_color(border)
            .child(
                div()
                    .size(px(76.))
                    .rounded_2xl()
                    .bg(soft)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(
                        Icon::new(IconName::GalleryVerticalEnd)
                            .with_size(px(34.))
                            .text_color(accent),
                    ),
            )
            .child(
                v_flex()
                    .items_center()
                    .gap_2()
                    .child(div().text_xl().font_semibold().child("把图片拖到这里"))
                    .child(
                        div()
                            .text_sm()
                            .text_color(muted_fg)
                            .child("松开鼠标即可加入队列，支持一次拖入多张图片或整个文件夹"),
                    ),
            )
            .child(
                Button::new("pick")
                    .primary()
                    .large()
                    .label("选择图片")
                    .icon(IconName::Inbox)
                    .on_click(cx.listener(|this, _, window, cx| this.pick_files(window, cx))),
            )
            .child(
                h_flex()
                    .flex_wrap()
                    .justify_center()
                    .gap_2()
                    .children(FORMAT_LABELS.map(|label| chip(label, muted_fg, chip_bg))),
            )
            .into_any_element()
    }

    fn render_grid(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex_1()
            .min_h_0()
            .w_full()
            .overflow_y_scrollbar()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .w_full()
                    .gap_3()
                    .content_start()
                    .children(
                        self.items
                            .iter()
                            .enumerate()
                            .map(|(index, item)| self.render_card(index, item, cx)),
                    ),
            )
            .into_any_element()
    }

    fn render_card(&self, index: usize, item: &ImageItem, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, muted, muted_fg, card_bg) = (
            theme.border,
            theme.muted,
            theme.muted_foreground,
            theme.background,
        );
        let is_first = index == 0;
        let is_last = index + 1 == self.items.len();

        let preview = match (&item.thumb, item.failed) {
            (Some(thumb), _) => img(thumb.clone())
                .object_fit(ObjectFit::Contain)
                .size_full()
                .into_any_element(),
            (None, true) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .gap_1()
                .child(
                    Icon::new(IconName::CircleAlert)
                        .with_size(px(20.))
                        .text_color(theme.danger),
                )
                .child(div().text_xs().text_color(muted_fg).child("无法预览"))
                .into_any_element(),
            (None, false) => v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(Spinner::new().small())
                .into_any_element(),
        };

        let meta = match item.src_dims {
            Some((width, height)) => format!("{width}×{height} · {}", human_size(item.bytes)),
            None => human_size(item.bytes),
        };

        let hover_border = theme.primary;

        v_flex()
            .id(ElementId::Name(format!("card-{index}").into()))
            .w(px(186.))
            .flex_shrink_0()
            .rounded_xl()
            .border_1()
            .border_color(border)
            .bg(card_bg)
            .overflow_hidden()
            .hover(move |style| style.border_color(hover_border))
            .child(
                div()
                    .relative()
                    .h(px(148.))
                    .w_full()
                    .p_2()
                    .bg(muted.opacity(0.45))
                    .child(
                        div()
                            .size_full()
                            .rounded_lg()
                            .overflow_hidden()
                            .child(preview),
                    )
                    .child(
                        div()
                            .absolute()
                            .top_2()
                            .left_2()
                            .px_1p5()
                            .py_0p5()
                            .rounded_md()
                            .bg(black().opacity(0.55))
                            .text_xs()
                            .text_color(white())
                            .child(format!("{}", index + 1)),
                    ),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .p_2()
                    .gap_1()
                    .child(div().text_xs().font_medium().truncate().child(item.name.clone()))
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_xs().text_color(muted_fg).child(meta))
                            .child(
                                h_flex()
                                    .items_center()
                                    .gap_0p5()
                                    .child(
                                        Button::new(ElementId::Name(format!("up-{index}").into()))
                                            .ghost()
                                            .xsmall()
                                            .icon(IconName::ArrowUp)
                                            .disabled(is_first)
                                            .tooltip("上移")
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.move_item(index, -1, cx)
                                            })),
                                    )
                                    .child(
                                        Button::new(ElementId::Name(
                                            format!("down-{index}").into(),
                                        ))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::ArrowDown)
                                        .disabled(is_last)
                                        .tooltip("下移")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.move_item(index, 1, cx)
                                        })),
                                    )
                                    .child(
                                        Button::new(ElementId::Name(
                                            format!("del-{index}").into(),
                                        ))
                                        .ghost()
                                        .xsmall()
                                        .icon(IconName::Close)
                                        .tooltip("移除")
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.remove_item(index, cx)
                                        })),
                                    ),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_sidebar(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (border, sidebar_bg, muted_fg) =
            (theme.border, theme.sidebar, theme.muted_foreground);

        let page = segmented(
            "page",
            &PageSize::ALL,
            self.options.page,
            cx,
            |this, value| this.options.page = value,
        );
        let orientation = segmented(
            "orientation",
            &Orientation::ALL,
            self.options.orientation,
            cx,
            |this, value| this.options.orientation = value,
        );
        let fit = segmented("fit", &FitMode::ALL, self.options.fit, cx, |this, value| {
            this.options.fit = value
        });
        let margin = segmented(
            "margin",
            &Margin::ALL,
            self.options.margin,
            cx,
            |this, value| this.options.margin = value,
        );

        let compress = self.options.compress;
        let pages = self.items.len();
        let status = self.render_status(cx);

        v_flex()
            .w(px(324.))
            .h_full()
            .flex_shrink_0()
            .bg(sidebar_bg)
            .border_l_1()
            .border_color(border)
            .child(
                v_flex()
                    .flex_1()
                    .min_h_0()
                    .p_4()
                    .gap_5()
                    .overflow_y_scrollbar()
                    .child(setting_label("页面尺寸", muted_fg))
                    .child(page)
                    .child(setting_label("纸张方向", muted_fg))
                    .child(orientation)
                    .child(setting_label("图片适配", muted_fg))
                    .child(fit)
                    .child(setting_label("页边距", muted_fg))
                    .child(margin)
                    .child(Separator::horizontal())
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(
                                v_flex()
                                    .gap_0p5()
                                    .child(div().text_sm().font_medium().child("压缩图片"))
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(muted_fg)
                                            .child("转为 JPEG，体积更小，透明处铺白"),
                                    ),
                            )
                            .child(
                                Switch::new("compress")
                                    .checked(compress)
                                    .on_click(cx.listener(|this, checked: &bool, _, cx| {
                                        this.options.compress = *checked;
                                        cx.notify();
                                    })),
                            ),
                    ),
            )
            .child(
                v_flex()
                    .flex_shrink_0()
                    .p_4()
                    .gap_3()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        h_flex()
                            .items_center()
                            .justify_between()
                            .child(div().text_xs().text_color(muted_fg).child("输出页数"))
                            .child(
                                div()
                                    .text_sm()
                                    .font_semibold()
                                    .child(format!("{pages} 页")),
                            ),
                    )
                    .child(
                        Button::new("export")
                            .primary()
                            .large()
                            .w_full()
                            .label("导出 PDF")
                            .icon(IconName::FileText)
                            .loading(self.busy)
                            .disabled(self.items.is_empty() || self.busy)
                            .on_click(cx.listener(|this, _, window, cx| this.export(window, cx))),
                    )
                    .child(status),
            )
            .into_any_element()
    }

    fn render_status(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted_fg, success) = (theme.muted_foreground, theme.success);

        match &self.status {
            Status::Idle => div()
                .text_xs()
                .text_color(muted_fg)
                .child("每张图片生成一页，按列表顺序排列")
                .into_any_element(),
            Status::Working => h_flex()
                .items_center()
                .gap_2()
                .child(Spinner::new().small())
                .child(div().text_xs().text_color(muted_fg).child("正在生成 PDF…"))
                .into_any_element(),
            Status::Done {
                file,
                pages,
                skipped,
                dir,
            } => {
                let dir = dir.clone();
                let summary = if *skipped > 0 {
                    format!("已导出 {pages} 页 · {file}（跳过 {skipped} 张）")
                } else {
                    format!("已导出 {pages} 页 · {file}")
                };
                v_flex()
                    .gap_2()
                    .child(
                        h_flex()
                            .items_center()
                            .gap_2()
                            .child(
                                Icon::new(IconName::CircleCheck)
                                    .with_size(px(14.))
                                    .text_color(success),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .text_xs()
                                    .text_color(muted_fg)
                                    .child(summary),
                            ),
                    )
                    .child(
                        Button::new("open-dir")
                            .ghost()
                            .xsmall()
                            .label("打开所在文件夹")
                            .icon(IconName::FolderOpen)
                            .on_click(move |_: &ClickEvent, _: &mut Window, _: &mut App| {
                                open_folder(&dir);
                            }),
                    )
                    .into_any_element()
            }
            Status::Error(message) => div()
                .text_xs()
                .text_color(theme.danger)
                .child(message.clone())
                .into_any_element(),
        }
    }
}

impl Render for MainView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.pending.is_empty() {
            let pending = std::mem::take(&mut self.pending);
            cx.defer_in(window, move |this, _window, cx| this.add_files(pending, cx));
        }

        let theme = cx.theme();
        let (background, foreground, accent) = (theme.background, theme.foreground, theme.primary);

        let title_bar = self.render_title_bar(cx);
        let main = self.render_main(window, cx);
        let sidebar = self.render_sidebar(cx);

        v_flex()
            .id("app-root")
            .size_full()
            .bg(background)
            .text_color(foreground)
            // 整个窗口都是放置区：拖到侧边栏或标题栏也能加入。
            .on_drop::<ExternalPaths>(cx.listener(
                |this, paths: &ExternalPaths, _window, cx| {
                    this.add_files(paths.paths().to_vec(), cx);
                },
            ))
            .drag_over::<ExternalPaths>(move |style, _, _, _| {
                style.border_2().border_color(accent)
            })
            .child(title_bar)
            .child(
                h_flex()
                    .flex_1()
                    .min_h_0()
                    .items_stretch()
                    .child(main)
                    .child(sidebar),
            )
    }
}

// ---------------------------------------------------------------- 辅助

/// 分段选择器。
fn segmented<T, F>(
    id: &'static str,
    values: &[(T, &'static str)],
    selected: T,
    cx: &mut Context<MainView>,
    on_select: F,
) -> AnyElement
where
    T: Copy + PartialEq + 'static,
    F: Fn(&mut MainView, T) + Clone + 'static,
{
    let background = cx.theme().muted;
    let mut row = h_flex()
        .p_0p5()
        .gap_0p5()
        .rounded_lg()
        .bg(background)
        .w_full();

    for (value, label) in values {
        let value = *value;
        let on_select = on_select.clone();
        let is_selected = value == selected;
        let mut button =
            Button::new(ElementId::Name(format!("{id}-{label}").into()))
                .label(*label)
                .xsmall()
                .flex_1();
        if is_selected {
            button = button.primary();
        } else {
            button = button.ghost();
        }
        row = row.child(button.on_click(
            cx.listener(move |this, _: &ClickEvent, _window, cx| {
                on_select(this, value);
                cx.notify();
            }),
        ));
    }

    row.into_any_element()
}

fn setting_label(label: &'static str, color: Hsla) -> AnyElement {
    div()
        .text_xs()
        .font_medium()
        .text_color(color)
        .child(label)
        .into_any_element()
}

fn chip(label: &'static str, fg: Hsla, bg: Hsla) -> AnyElement {
    div()
        .px_2()
        .py_0p5()
        .rounded_full()
        .bg(bg)
        .text_xs()
        .font_medium()
        .text_color(fg)
        .child(label)
        .into_any_element()
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| SUPPORTED_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str()))
        .unwrap_or(false)
}

/// 展开文件夹并过滤出支持的图片，保持给定顺序。
fn collect_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(path)
                .into_iter()
                .flatten()
                .flatten()
                .map(|entry| entry.path())
                .filter(|entry| entry.is_file() && is_supported(entry))
                .collect();
            entries.sort_by_key(|entry| {
                entry
                    .file_name()
                    .map(|name| name.to_string_lossy().to_lowercase())
                    .unwrap_or_default()
            });
            files.extend(entries);
        } else if path.is_file() && is_supported(path) {
            files.push(path.clone());
        }
    }
    files
}

fn suggested_name(items: &[ImageItem]) -> String {
    let stem = items
        .first()
        .and_then(|item| item.path.file_stem())
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "images".to_string());
    if items.len() > 1 {
        format!("{stem}-等{}张.pdf", items.len())
    } else {
        format!("{stem}.pdf")
    }
}

fn default_dir() -> PathBuf {
    if let Some(profile) = std::env::var_os("USERPROFILE") {
        let pictures = PathBuf::from(&profile).join("Pictures");
        if pictures.is_dir() {
            return pictures;
        }
        return PathBuf::from(profile);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home);
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

fn toggle_theme(window: &mut Window, cx: &mut App) {
    let next = if cx.theme().is_dark() {
        ThemeMode::Light
    } else {
        ThemeMode::Dark
    };
    Theme::change(next, Some(window), cx);
}

fn open_folder(dir: &Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer").arg(dir).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(dir).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
    }
}
