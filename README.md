# WeChat-Bot

一个 Windows 上的微信机器人开发模板，使用 Rust 开发。

基于 [WeChatFerry](https://github.com/lich0821/WeChatFerry) 的 [v39.2.4](https://github.com/lich0821/WeChatFerry/releases/tag/v39.2.4), 和 [protobuf](https://github.com/protocolbuffers/protobuf/releases)。

<details><summary><font color="red" size="12">免责声明【必读】</font></summary>

本工具仅供学习和技术研究使用，不得用于任何商业或非法行为，否则后果自负。

本工具的作者不对本工具的安全性、完整性、可靠性、有效性、正确性或适用性做任何明示或暗示的保证，也不对本工具的使用或滥用造成的任何直接或间接的损失、责任、索赔、要求或诉讼承担任何责任。

本工具的作者保留随时修改、更新、删除或终止本工具的权利，无需事先通知或承担任何义务。

本工具的使用者应遵守相关法律法规，尊重微信的版权和隐私，不得侵犯微信或其他第三方的合法权益，不得从事任何违法或不道德的行为。

本工具的使用者在下载、安装、运行或使用本工具时，即表示已阅读并同意本免责声明。如有异议，请立即停止使用本工具，并删除所有相关文件。

</details>

## 使用方法

参考 `main.rs` 中的例子，自行修改即可。



## 已知问题

1. API 接口主要参考了[原始 rust client](https://github.com/lich0821/WeChatFerry/tree/master/clients/rust)，调整了个别接口以改善易用性，大部分保持不变。
2. 运行前需要提前启动微信，否则可能会出现一些意外情况。
3. 编译时，如果遇到文件拷贝失败警告，是因为注入的 dll 被微信占用导致。可以不用关心，或关闭微信重开即可解决。