# 图片转 PDF

基于 [GPUI Kit](https://gpui-kit.com)（GPUI + GPUI Component）的桌面小工具：把图片拖进窗口，调整顺序和页面参数，一键导出 PDF。

## 功能

- **拖放导入**：把图片或整个文件夹拖进窗口即可加入队列；也支持「选择图片」按钮调用系统文件对话框，或通过命令行 / 打开方式直接带入文件。
- **常用格式**：PNG、JPG/JPEG、WebP（含动图首帧）、GIF、BMP、TIFF、ICO、TGA、QOI、PNM、DDS、HDR。
- **顺序整理**：卡片式预览，支持上移 / 下移、单个移除、按名称排序、清空；索引即页序。
- **页面设置**：原图（跟随图片比例）/ A4 / A3 / A5 / Letter，自动 / 纵向 / 横向，适应 / 填充（裁切）/ 拉伸，四档页边距。
- **无损优先**：基线 JPEG 直接以 `DCTDecode` 原样嵌入，不做二次编码；其他格式使用 Flate 无损压缩，并保留 PNG 透明通道（`SMask`）。
- **压缩选项**：开启后统一转成 JPEG（质量 86、透明铺白），体积明显更小。
- **明暗主题**：标题栏右上角一键切换。

## 运行

```bash
cargo run --release
# 或者直接带入图片
cargo run --release -- D:\photos\a.png D:\photos\b.webp
```

首次编译需要构建 GPUI，耗时较长。

## 结构

| 文件 | 说明 |
| --- | --- |
| `src/main.rs` | 应用入口、窗口选项与命令行参数 |
| `src/app.rs` | 主界面：拖放、卡片网格、侧边栏设置、导出流程 |
| `src/pdf.rs` | PDF 生成：页面布局、图片编码（JPEG 直通 / Flate / SMask） |
| `src/thumbs.rs` | 后台解码缩略图，转换成 GPUI 可渲染的 BGRA 图像 |

## 测试

```bash
cargo test
```

覆盖多格式导出、压缩体积、页面方向与比例、JPEG 直通、坏文件跳过；生成的 PDF 会用 `lopdf` 解析校验页数。
