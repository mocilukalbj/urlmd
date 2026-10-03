# urlmd 0.1.3 文档站点验证

验证日期：2026-10-03。Linux x86_64，Rust 1.98.1。

40 项自动测试通过，另行执行的网页快照结构检查通过。15 个域名、20 个抓取场景全部成功；每个响应离线重放 keep/relative/text，共 60 份输出。release 与 debug 输出逐字节一致。

表格回归验证行头/列头的位置、混合 th/td、空单元格、caption、多个 tbody、tfoot、合并单元格、嵌套表格及代码。真实页面检查了正文中 270 个非空单元格的文字是否出现在转换结果中（忽略空白，不等于逐格视觉布局对照），并核对 998 个代码块在三种链接模式下完全相同。

| 场景 | 内容类型 | 代码块 | 核对单元格 | keep 字符 | text 字符 |
| --- | --- | ---: | ---: | ---: | ---: |
| [mdn-content-type](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Content-Type) | text/html | 9 | 11 | 10751 | 5840 |
| [mdn-set-cookie](https://developer.mozilla.org/en-US/docs/Web/HTTP/Reference/Headers/Set-Cookie) | text/html | 8 | 8 | 21591 | 14221 |
| [mdn-display](https://developer.mozilla.org/en-US/docs/Web/CSS/Reference/Properties/display) | text/html | 16 | 13 | 36807 | 20413 |
| [python-json](https://docs.python.org/3/library/json.html) | text/html | 15 | 34 | 33549 | 25422 |
| [python-argparse](https://docs.python.org/3/library/argparse.html) | text/html | 94 | 0 | 104946 | 82347 |
| [rust-vec](https://doc.rust-lang.org/std/vec/struct.Vec.html) | text/html | 278 | 0 | 417112 | 232468 |
| [rust-option](https://doc.rust-lang.org/std/option/enum.Option.html) | text/html | 74 | 0 | 108560 | 45779 |
| [docs-rs-serde](https://docs.rs/serde/latest/serde/trait.Serialize.html) | text/html | 2 | 0 | 129135 | 25210 |
| [react-native](https://react.dev/learn) | text/plain | 27 | 0 | 15835 | 14855 |
| [react-html](https://react.dev/learn) | text/html | 24 | 0 | 17384 | 15719 |
| [typescript](https://www.typescriptlang.org/docs/handbook/2/everyday-types.html) | text/html | 42 | 6 | 30431 | 27428 |
| [vue](https://vuejs.org/guide/essentials/reactivity-fundamentals.html) | text/html | 36 | 0 | 19859 | 17405 |
| [go-http](https://pkg.go.dev/net/http) | text/html | 230 | 25 | 202986 | 159550 |
| [fastapi](https://fastapi.tiangolo.com/tutorial/body/) | text/html | 8 | 0 | 8251 | 7940 |
| [django](https://docs.djangoproject.com/en/5.2/ref/models/fields/) | text/html | 51 | 6 | 140255 | 86012 |
| [kubernetes](https://kubernetes.io/docs/concepts/services-networking/service/) | text/html | 19 | 8 | 50015 | 42474 |
| [postgresql](https://www.postgresql.org/docs/current/datatype-numeric.html) | text/html | 9 | 44 | 17034 | 15794 |
| [sqlite](https://www.sqlite.org/lang_createtable.html) | text/html | 1 | 12 | 22482 | 18228 |
| [wikipedia](https://en.wikipedia.org/wiki/HTTP) | text/html | 5 | 103 | 106071 | 55992 |
| [openai](https://developers.openai.com/api/docs/guides/tools-web-search) | text/markdown | 50 | 0 | 47392 | 46761 |

字符数不包含来源 front matter，不是 token 数。

与 0.1.2 使用同一份原始响应对照：19 个场景的既有代码内容保留，其中 React HTML 的 17 个代码块和 MDN display 的 1 个代码块恢复了 HTML br 对应的换行；TypeScript 的代码修复单独核对 42 个 pre 对应的代码块数量，并逐一验证 div.line 示例。

MDN Content-Type 当前属性表分类是 Representation header；历史 Request header/Response header 的混合行头形式仍有独立回归测试。rustdoc 标题徽章中的孤立 Write 和 MDN Baseline 控件不再输出，正文的 Write 实现、兼容性说明仍保留。

本机 release 离线转换耗时（每页独立启动 CLI，运行 5 次的中位数，包含读文件及转换，不含网络；不是跨机器性能承诺）：

| 页面 | 原始字节 | 毫秒 |
| --- | ---: | ---: |
| mdn-content-type | 222519 | 12.0 |
| rust-vec | 955558 | 111.0 |
| typescript | 321570 | 27.3 |
| wikipedia | 604570 | 69.9 |

已知边界：复杂表格使用逐行列表保留内容与 span 标记，不复原视觉网格；不执行 JavaScript、不递归抓站，也不解释 SVG/canvas 或图片（例如 SQLite 的 SVG 语法图）。站点模板和正文会变化，关键词与样本通过不代表任意页面完整无损。

复现方式见 README 的 scripts/check_sites.py 和 tests/site_snapshots.rs。原始网页保存在本机审计目录，没有提交到仓库。
