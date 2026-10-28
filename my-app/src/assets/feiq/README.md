# 飞秋内置表情资源

96 个 GIF 及符号/中文名称映射参考 [compilelife/feiq](https://github.com/compilelife/feiq) 的 Qt 飞秋项目：
`D:\sources\feiq\emoji.cpp`、`emoji.h`、`res\face\`。
按该项目 readme 要求保留来源说明。

这些 GIF 已与本机飞秋 `CustomFace\1.gif` 至 `96.gif` 逐个校验，SHA-256 均相同；
也与 PythonFeiQ 的 `GIF_451.gif` 至 `GIF_546.gif` 相同。
只复制内置表情资源，不加载或发送飞秋配置文件及缓存图。

映射和静态资源导入由 `tools/import-feiq-emojis.cjs` 生成。部署时所有 GIF 随应用打包，
不依赖上述本机目录。表情使用原始符号作为消息内容，例如微笑是 `/:)`；GIF 只用于本地显示。
