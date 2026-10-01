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

默认发送 `Accept: text/markdown`。站点返回原生 Markdown 时直接保存；返回 HTML 时选择正文、清理脚本与隐藏元素，再转换为 Markdown。下载完整响应后处理，不按 HTML 字符前缀截断。

默认 User-Agent 为 `urlmd/0.1.1`。可以用 `--user-agent` 按站点要求覆盖；空值、换行及非法 HTTP 头值会在请求前报错。Wikimedia 要求工具提供描述性 UA 和联系方式，并明确不建议机器人复制浏览器 UA，详见 [官方 User-Agent 政策](https://foundation.wikimedia.org/wiki/Policy:User-Agent_policy)。访问这类站点时，将下面的联系方式替换为你自己的真实联系地址：

```sh
urlmd https://en.wikipedia.org/wiki/Markdown \
  --user-agent 'urlmd/0.1.1 (https://your-domain.example/contact)' -o wikipedia.md
```

对于需要浏览器 UA 的其他站点，也可以传入相应 UA；403 还可能来自访问权限、频率限制或站点验证，覆盖 UA 并不能保证解决。

OpenAI 文档站支持这种内容协商，也提供追加 `.md` 的地址。本次检查中，`/api/docs` 的原生 Markdown 是文档目录，HTML 首页则包含首页卡片和快速示例。需要 HTML 页面的内容时使用 `--html`。

```sh
urlmd https://developers.openai.com/api/docs --html -o docs-homepage.md
urlmd https://developers.openai.com/api/docs/guides/tools-web-search -o web-search.md
```

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
| `--encoding gbk` | 显式指定字符集；默认使用 BOM、HTTP charset、早期 HTML meta 或 UTF-8 |
| `--max-bytes 20971520` | 默认最多读取 20 MiB 响应正文，超限报错，不生成截断结果 |
| `--timeout 30` | 网络超时秒数 |
| `--user-agent 'UA'` | 覆盖 URL 请求的 User-Agent，默认为 `urlmd/0.1.1` |
| `--input -` | 从 stdin 读取 HTML |
| `--help` | 查看全部用法 |

HTML 模式会解析相对链接与图片地址，支持 HTML `base`，采用重定向后的最终 URL 为基准。优先选择常见文档正文容器、article、main，最后回退 body。清理隐藏属性、aria-hidden 和内联隐藏样式；仅凭 CSS class 名不会删除内容，因此会保留某些站点备用语言 Tab 的示例。代码语言支持 `language-*` 与 `data-language`。原生 Markdown 模式保留站点提供的排版与链接。

这是单页转换器。不会递归抓取整站，也不会执行网页 JavaScript。CSS 正文规则不可能适合所有网站，特殊站点可以指定 `--selector`；只有浏览器运行后才出现的正文需要另行获取渲染后的 HTML，再用 `--input` 转换。图片在 Markdown 中保留引用，不进行 OCR。

仓库包括源代码、锁定依赖和测试。需要单独构建或运行测试时：

```sh
cargo build --release --locked
cargo test --locked
# 构建后程序位于 target/release/urlmd
```

本次使用 Rust 1.98.1 验证。转换层使用 [htmd](https://github.com/letmutex/htmd)，DOM 操作使用 [dom_query](https://github.com/niklak/dom_query)，HTTP 使用 [reqwest](https://github.com/seanmonstar/reqwest)。完整依赖版本见 Cargo.lock。

验证结果：4 项字符编码/代码格式测试、15 项 CLI 集成测试全部通过。集成测试覆盖长脚本前置、HTML 正文/表格、原生 Markdown、重定向与链接、下载超限、HTTP 错误、离线输入、CSS 选择器、留档与输出路径冲突，以及默认/自定义 UA、非法 UA 拒绝、stdin 管道的代码格式与相对链接、stdin 超限和空输入。2026-10-01 对 OpenAI 文档首页的 HTML 转换检查保留了 8 个代码块的原始文本、换行、空行和缩进；正文链接经抽查全部保留。

## 卸载

只删除当前用户安装的程序即可，已有的 `PATH` 配置可以保留供其他工具使用：

```sh
rm "$HOME/.local/bin/urlmd"
```
