mod app;
mod pdf;
mod thumbs;

use std::path::PathBuf;

use gpui_kit::component::TitleBar;
use gpui_kit::*;

fn main() {
    // 支持通过命令行或“打开方式”直接带入图片。
    let initial: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();

    gpui_kit::application()
        .with_assets(gpui_kit::assets::Assets)
        .run(move |cx: &mut App| {
            gpui_kit::init(cx);

            let mut titlebar = TitleBar::title_bar_options();
            titlebar.title = Some("图片转 PDF".into());

            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds::centered(
                    None,
                    size(px(960.), px(620.)),
                    cx,
                ))),
                window_min_size: Some(size(px(780.), px(520.))),
                titlebar: Some(titlebar),
                ..TitleBar::window_options()
            };

            gpui_kit::open_window(options, cx, move |_, cx| {
                let initial = initial.clone();
                cx.new(move |cx| app::MainView::new(initial, cx))
            })
            .expect("failed to open window");
        });
}
