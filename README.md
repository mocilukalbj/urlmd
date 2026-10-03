# urlmd

输入网址，保存 Markdown 的本地 Rust CLI。运行已编译程序不需要 Python、Node、API key 或模型调用。

## 安装与 PATH

需要 Rust 工具链。从 GitHub 获取源码并安装到当前用户的 `~/.local/bin`：

```sh
git clone https://github.com/mocilukalbj/urlmd.git
cd urlmd
cargo install --path . --locked --root "$HOME/.local"
```

如果使用附带 `bin/urlmd` 的 Linux 预编译归档，解压后也可以在归档目录直接安装：

```sh
mkdir -p "$HOME/.local/bin"
install -m 755 bin/urlmd "$HOME/.local/bin/urlmd"
```

`PATH` 是 shell 查找命令的环境变量。若 `~/.local/bin` 已在 `PATH` 中，安装后即可直接运行 `urlmd`，无需重复修改 shell 配置。否则在当前终端运行以下片段，使它立即生效；这个片段不会重复添加已有路径：

```sh
case ":$PATH:" in
  *":$HOME/.local/bin:"*) ;;
  *) export PATH="$HOME/.local/bin:$PATH" ;;
esac
```

使用 Bash 时，如需新终端也能直接运行，将同一片段加入 `~/.bashrc`。已有配置包含这个路径时不用再添加。保存后可运行 `source "$HOME/.bashrc"`，或重新打开终端。

验证安装：

```sh
command -v urlmd
urlmd --version
```

也可以构建后使用 `./target/release/urlmd`，或在预编译归档中使用 `./bin/urlmd`；下文命令中的 `urlmd` 均可替换为对应路径。

## 使用

```sh
urlmd https://developers.openai.com/api/docs -o docs.md
```

不指定 `-o` 时，Markdown 写入 stdout，可以重定向或接管道；状态和错误写入 stderr。

```sh
urlmd https://example.com/guide > guide.md
```

默认发送 `Accept: text/markdown`。站点返回原生 Markdown 时直接保存；非 HTML 的 `text/plain` 也按 Markdown 兼容文本保留，不要求网址以 `.md` 结尾（例如 React 文档）。来源记录保留实际 Content-Type，并将这类响应的 format 标为 `plain-text`。返回 HTML 时选择正文、清理脚本与隐藏元素，再转换为 Markdown。下载完整响应后处理，不按 HTML 字符前缀截断。

默认 User-Agent 为 `urlmd/0.1.3`。可以用 `--user-agent` 按站点要求覆盖；空值、换行及非法 HTTP 头值会在请求前报错。Wikimedia 要求工具提供描述性 UA 和联系方式，并明确不建议机器人复制浏览器 UA，详见 [官方 User-Agent 政策](https://foundation.wikimedia.org/wiki/Policy:User-Agent_policy)。访问这类站点时，将下面的联系方式替换为你自己的真实联系地址：

```sh
urlmd https://en.wikipedia.org/wiki/Markdown \
  --user-agent 'urlmd/0.1.3 (https://your-domain.example/contact)' -o wikipedia.md
```

对于需要浏览器 UA 的其他站点，也可以传入相应 UA；403 还可能来自访问权限、频率限制或站点验证，覆盖 UA 并不能保证解决。

OpenAI 文档站支持这种内容协商，也提供追加 `.md` 的地址。本次检查中，`/api/docs` 的原生 Markdown 是文档目录，HTML 首页则包含首页卡片和快速示例。需要 HTML 页面的内容时使用 `--html`。

```sh
urlmd https://developers.openai.com/api/docs --html -o docs-homepage.md
urlmd https://developers.openai.com/api/docs/guides/tools-web-search -o web-search.md
```

Agent 只需要阅读正文时，可以省略超链接目标和来源 front matter：

```sh
urlmd https://react.dev/learn --links text --no-metadata
```

需要继续访问文档链接时，可以缩短同站 URL，并保留来源 URL 供解析：

```sh
urlmd https://doc.rust-lang.org/std/vec/struct.Vec.html --links relative -o vec.md
```

| 链接模式 | 行为 |
|---|---|
| `--links keep`（默认） | 保留超链接；HTML 中解析成绝对 URL，原生 Markdown 保留原始写法 |
| `--links relative` | 同协议、主机和端口的绝对 URL 缩短为 `/path?query#fragment`；同页锚点缩短为 `#fragment`；外站及已有相对链接保持原样 |
| `--links text` | 超链接保留标签文字和格式，移除目标及标题；删除不再使用的引用定义，共享给图片的定义保留 |

链接模式同时适用于 HTML 转换结果和原生 Markdown，来源记录包含所选模式。图片、代码块、行内代码和正文中直接写出的 URL 保留；原生 Markdown 内嵌 HTML/JSX 的属性保持原样。`relative` 模式离线使用时应提供 `--base-url`，否则保留原链接。使用 `--no-metadata` 时，调用方需要自行保存来源 URL。

保存原始响应和来源信息，方便之后重新提取：

```sh
urlmd https://example.com/guide --html \
  --save-source page.html -o page.md
```

此命令同时保存 `page.html.meta.json`，包含请求地址、最终地址、抓取与转换时间、Content-Type、实际解码字符集、SHA-256、正文选择规则和转换器版本。`--save-source` 保存响应正文原始字节，HTTP 传输压缩会由客户端解压。下载完成后即留档，之后提取失败时原始响应仍在，sidecar 会记录失败原因。本地文件转换不声称知道原网页抓取时间。

离线转换已保存的 HTML：

```sh
urlmd --input page.html --base-url https://example.com/guide -o page.md
urlmd --input page.html --base-url https://example.com/guide \
  --selector 'main .content' -o content.md
```

也可以直接从管道读取 HTML：

```sh
set -o pipefail
curl -fsSL -H 'Accept: text/html' https://developers.openai.com/api/docs |
  urlmd --input - --base-url https://developers.openai.com/api/docs -o docs-from-pipe.md
```

`--input -` 已支持 stdin，不需要再增加开关。`--base-url` 用于解析相对链接；若 curl 跟随了重定向，应填写最终页面地址。此时下载由 curl 完成，`urlmd --user-agent` 不影响 curl；需要设置 curl 的 UA 时使用 `curl -A '你的 UA'`。stdin 不携带 HTTP 响应头，字符集由 HTML meta/BOM、`--encoding` 或 UTF-8 决定。`pipefail` 使上游 curl 失败时整条管道也返回失败。

其他常用选项：

| 选项 | 用途 |
|---|---|
| `--selector 'CSS'` | 指定正文元素；多个匹配会合并；无匹配或非法 CSS 报错；在线输入会请求 HTML |
| `--whole-page` | 转换整个 body，保留导航；在线输入会请求 HTML |
| `--no-metadata` | 不在 Markdown 开头添加 YAML 来源信息 |
| `--links keep\|relative\|text` | 保留链接、缩短同站 URL，或只保留链接文字；默认 keep |
| `--encoding gbk` | 显式指定字符集；默认使用 BOM、HTTP charset、早期 HTML meta 或 UTF-8 |
| `--max-bytes 20971520` | 默认最多读取 20 MiB 响应正文，超限报错，不生成截断结果 |
| `--timeout 30` | 网络超时秒数 |
| `--user-agent 'UA'` | 覆盖 URL 请求的 User-Agent，默认为 `urlmd/0.1.3` |
| `--input -` | 从 stdin 读取 HTML |
| `--help` | 查看全部用法 |

HTML 模式会解析相对链接与图片地址，支持 HTML `base`，采用重定向后的最终 URL 为基准。优先选择常见文档正文容器、article、main，最后回退 body。清理隐藏属性、aria-hidden 和内联隐藏样式；仅凭 `hidden` 这个 CSS class 名不会删除内容，因此会保留某些站点备用语言 Tab 的示例。

Python/rustdoc 的装饰性 `¶/§` 自锚点、rustdoc 的复制路径按钮、展开提示和 playground 运行按钮会按 DOM 结构清除，标题和折叠区域正文保留。rustdoc 标题下的 trait 徽章及 MDN Baseline/调查控件也会清除，正文中的 trait 实现和兼容性说明保留。MDN 的语言标签会写入代码围栏，代码语言支持 `language-*`、`data-language`、`brush:` 和 rustdoc 的 `rust` 类。裸 `<pre>`、`<code><pre>` 嵌套和代码内的 `<br>` 换行也会保留。TypeScript/Twoslash 的 `div.line` 会恢复为逐行代码，诊断信息放在代码块后，语言标签和 Try 控件不混入代码。原生 Markdown 默认保留站点提供的排版与链接，指定链接模式时仅修改解析到的 Markdown 超链接。

普通表格按原始 `th`/`td` 顺序转换，不再把行头误当作列头而丢失内容。没有明确列头时添加空表头，保留第一行数据；支持混合单元格、caption、多个 tbody、tfoot 和空单元格。包含合并单元格、多行表头、嵌套表格或代码块/列表的复杂表格，转换为逐行、逐单元格列表，并标注 `rowspan`/`colspan`，保留内容顺序；不模拟合并后的视觉网格。

例如：

```html
<table><tr><th>K</th><td>V1</td></tr><tr><th>K2</th><td>V2</td></tr></table>
```

会得到：

```markdown
|  |  |
| --- | --- |
| K | V1 |
| K2 | V2 |
```

默认正文选择还覆盖 TypeScript 手册、Wikipedia、PostgreSQL 和 SQLite 的常见内容容器。`--selector`/`--whole-page` 可覆盖默认范围。SVG/canvas 图形和纯浏览器渲染内容不会转成文本；例如 SQLite 的 SVG 语法图仍需查看原页。

这是单页转换器。不会递归抓取整站，也不会执行网页 JavaScript。CSS 正文规则不可能适合所有网站，特殊站点可以指定 `--selector`；只有浏览器运行后才出现的正文需要另行获取渲染后的 HTML，再用 `--input` 转换。图片在 Markdown 中保留引用，不进行 OCR。

仓库包括源代码、锁定依赖和测试。需要单独构建或运行测试时：

```sh
cargo build --release --locked
cargo test --locked
# 构建后程序位于 target/release/urlmd
```

本次使用 Rust 1.98.1 验证。转换层使用 [htmd](https://github.com/letmutex/htmd)，DOM 操作使用 [dom_query](https://github.com/niklak/dom_query)，链接改写使用 [pulldown-cmark](https://github.com/pulldown-cmark/pulldown-cmark) 的源文本位置，HTTP 使用 [reqwest](https://github.com/seanmonstar/reqwest)。完整依赖版本见 Cargo.lock。

验证结果（0.1.3）：40 项自动测试通过（13 项单元测试、18 项 CLI 集成测试、9 项表格回归），另有 1 项需要本地网页快照的可选结构检查。覆盖编码、代码缩进/空行、控件清理、原生 Markdown/text/plain、三种链接模式、引用与图片、重定向、HTTP 错误、下载限制、stdin、CSS 选择器和留档冲突。

2026-10-03 实测 15 个文档域名、20 个场景、60 份链接模式输出：MDN、Python、Rust、docs.rs、React（原生与 HTML）、TypeScript、Vue、Go、FastAPI、Django、Kubernetes、PostgreSQL、SQLite、Wikipedia、OpenAI。检查正文关键词、270 个非空表格单元格的文本，以及 998 个代码块在三种链接模式下的一致性。快照比较使用 0.1.2；除 TypeScript 代码排版修复外，原有代码块内容保留，18 个代码块恢复了 `<br>` 对应的换行。TypeScript 单独对照 HTML 中的逐行代码核验。字符数不是 token 数；这些检查也不代表所有页面都能完整转换。

逐页结果、复现范围和耗时见 [验证报告](docs/verification-0.1.3.md)。

可选在线回归使用 Python 3 标准库驱动已编译 CLI（程序运行本身仍无需 Python）：

```sh
python3 scripts/check_sites.py --binary target/release/urlmd --out target/site-audit
URLMD_SITE_SNAPSHOTS="$PWD/target/site-audit" \
  cargo test --test site_snapshots -- --ignored --nocapture
# 重新检查同一批原始响应，不重复抓取：
python3 scripts/check_sites.py --binary target/release/urlmd --out target/site-audit --cached
```

站点列表位于 `scripts/sites.json`。报告和原始网页只保存在指定目录；抓取失败、转换失败或关键词缺失均返回非零状态。站点内容会变化，测试断言也需要根据真实页面维护。可用 `--baseline /path/to/older/urlmd` 增加旧版代码块对照。

## 卸载

只删除当前用户安装的程序即可，已有的 `PATH` 配置可以保留供其他工具使用：

```sh
rm "$HOME/.local/bin/urlmd"
```
