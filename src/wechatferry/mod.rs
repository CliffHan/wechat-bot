#![allow(dead_code)]

use anyhow::{anyhow, Result};
use log::{error, trace, warn};
use nng::options::{Options, RecvTimeout, SendTimeout};
use nng::Socket;
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use prost::Message;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

mod loader;
pub mod proto {
    tonic::include_proto!("wcf");
    tonic::include_proto!("roomdata");
}

// set in init(), and unset in uninit()
static CMD_PORT: Lazy<Mutex<u16>> = Lazy::new(|| Mutex::new(0));
// connect as caller requests, disconnect when uninit() or error happens
static CMD_SOCKET: Lazy<Arc<Mutex<Option<Socket>>>> = Lazy::new(|| Arc::new(Mutex::new(None)));
// set in enable_listen(), and unset in disable_listen()
static MSG_PORT: Lazy<Mutex<u16>> = Lazy::new(|| Mutex::new(0));
// lives in recv_msg_thread, and only one could live
static MSG_RECEIVING: Lazy<Arc<Mutex<()>>> = Lazy::new(|| Arc::new(Mutex::new(())));

const RECV_TIMEOUT: Duration = Duration::from_millis(5000);
const SEND_TIMEOUT: Duration = Duration::from_millis(5000);

pub struct CleanupHandler {
    auto_clean: bool,
}

impl Drop for CleanupHandler {
    fn drop(&mut self) {
        if self.auto_clean {
            uninit();
        }
    }
}

#[derive(Clone, Debug)]
pub enum Event {
    SdkDllLoaded,
    SdkInited(u16, bool),
    SdkDestroyed,
    CmdSocketConnected,
    CmdSocketDisconnected,
    MsgSocketConnected,
    MsgSocketDisconnected,
    MsgReceived(proto::WxMsg),
}

#[derive(Clone, Debug)]
pub struct UserInfo {
    pub wxid: String,
    pub name: String,
    pub mobile: String,
    pub home: String,
}

impl From<proto::UserInfo> for UserInfo {
    fn from(user_info: proto::UserInfo) -> Self {
        UserInfo { wxid: user_info.wxid, name: user_info.name, mobile: user_info.mobile, home: user_info.home }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ContactInfo {
    /// 微信ID
    pub wxid: String,
    /// 微信号
    pub alias: Option<String>,
    /// 删除标记
    pub del_flag: u8,
    /// 类型
    pub contact_type: u8,
    /// 备注
    pub remark: Option<String>,
    /// 昵称
    pub nick_name: Option<String>,
    /// 昵称拼音首字符
    pub py_initial: Option<String>,
    /// 昵称全拼
    pub quan_pin: Option<String>,
    /// 备注拼音首字母
    pub remark_py_initial: Option<String>,
    /// 备注全拼
    pub remark_quan_pin: Option<String>,
    /// 小头像
    pub small_head_url: Option<String>,
    /// 大头像
    pub big_head_url: Option<String>,
}

impl From<proto::DbRow> for ContactInfo {
    fn from(row: proto::DbRow) -> Self {
        let mut ci = ContactInfo::default();
        let from_utf8 = String::from_utf8; // to shorten code lines
        for field in row.fields {
            match field.column.as_str() {
                "UserName" => ci.wxid = from_utf8(field.content).unwrap_or_default(),
                "Alias" => ci.alias = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                "DelFlag" => ci.del_flag = field.content.first().copied().unwrap_or(0),
                "Type" => ci.contact_type = field.content.first().copied().unwrap_or(0),
                "Remark" => ci.remark = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                "NickName" => ci.nick_name = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                "PYInitial" => ci.py_initial = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                "QuanPin" => ci.quan_pin = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                "RemarkPYInitial" => ci.remark_py_initial = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                "RemarkQuanPin" => ci.remark_quan_pin = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                "smallHeadImgUrl" => ci.small_head_url = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                "bigHeadImgUrl" => ci.big_head_url = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                _ => {}
            }
        }
        ci
    }
}

#[derive(Clone, Debug, Default)]
pub struct ChatRoom {
    /// 群聊ID
    pub room_id: String,
    /// 群聊成员
    pub room_data: proto::RoomData,
    /// 群聊头像
    pub room_head_img_url: Option<String>,
    /// 公告
    pub room_announcement: Option<String>,
}

impl From<proto::DbRow> for ChatRoom {
    fn from(row: proto::DbRow) -> Self {
        let mut room = ChatRoom::default();
        let from_utf8 = String::from_utf8; // to shorten code lines
        for field in row.fields {
            match field.column.as_str() {
                "ChatRoomName" => room.room_id = from_utf8(field.content).unwrap_or_default(),
                "RoomData" => room.room_data = proto::RoomData::decode(field.content.as_slice()).unwrap_or_default(),
                "smallHeadImgUrl" => room.room_head_img_url = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                "Announcement" => room.room_announcement = from_utf8(field.content).ok().filter(|s| !s.is_empty()),
                _ => {}
            }
        }
        room
    }
}

pub type CallbackFn = Arc<Mutex<dyn FnMut(Event) + Send + 'static>>;
static EVENT_CALLBACK: Lazy<Mutex<Option<CallbackFn>>> = Lazy::new(|| Mutex::new(None));

fn exchange_message(socket: &Socket, msg: nng::Message) -> Result<nng::Message> {
    socket.send(msg).map_err(|e| anyhow!("send error, e: {:?}", e))?;
    Ok(socket.recv()?)
}

fn exchange_message_via_cmd_socket(msg: nng::Message) -> Result<nng::Message> {
    let mut cmd_socket_option = CMD_SOCKET.lock();
    if cmd_socket_option.is_none() {
        return Err(anyhow!("cmd_socket disconnected"));
    }
    let socket = cmd_socket_option.as_ref().unwrap();
    let exchange_result = exchange_message(socket, msg);
    if let Err(e) = exchange_result.as_ref() {
        let error = format!("failed to send or receive, error={:?}", e);
        error!("{}, disconnect cmd_socket", &error);
        *cmd_socket_option = None;
        send_event(Event::CmdSocketDisconnected);
        return Err(anyhow!(error));
    }
    exchange_result
}

fn run_cmd(func: i32, msg: Option<proto::request::Msg>) -> Result<proto::Response> {
    let req = proto::Request { func, msg };
    let mut buf = Vec::with_capacity(req.encoded_len());
    req.encode(&mut buf)?;
    let msg = nng::Message::from(&buf[..]);
    let msg_recv = exchange_message_via_cmd_socket(msg)?;
    Ok(proto::Response::decode(msg_recv.as_slice())?)
}

fn get_response_status_as_bool(response: &proto::Response) -> bool {
    match response.msg {
        Some(proto::response::Msg::Status(status)) => 1 == status,
        _ => false,
    }
}

fn send_event(event: Event) {
    let arc_callback = match EVENT_CALLBACK.lock().as_ref() {
        Some(p) => p.clone(),
        None => return,
    };
    arc_callback.lock()(event);
}

fn connect_socket(port: u16) -> Result<Socket> {
    let socket = Socket::new(nng::Protocol::Pair1)?;
    socket.set_opt::<RecvTimeout>(Some(RECV_TIMEOUT))?;
    socket.set_opt::<SendTimeout>(Some(SEND_TIMEOUT))?;
    let url = format!("tcp://127.0.0.1:{}", port);
    socket.dial(&url)?;
    Ok(socket)
}

fn recv_msg_thread(port: u16) {
    trace!("recv_msg_thread()");
    let _receiving = match MSG_RECEIVING.try_lock() {
        Some(v) => v,
        None => return, // cannot lock, which means there's another thread is still working
    };
    let socket = match connect_socket(port) {
        Ok(s) => s,
        Err(e) => {
            error!("cannot connect to msg socket, port {}, error: {}", port, e);
            return;
        }
    };
    send_event(Event::MsgSocketConnected);

    loop {
        match socket.recv() {
            Ok(mut msg) => {
                let response = match proto::Response::decode(msg.as_slice()) {
                    Ok(resp) => resp,
                    Err(e) => {
                        error!("received invalid msg, error={}", e);
                        continue;
                    }
                };
                msg.clear();
                if let Some(proto::response::Msg::Wxmsg(msg)) = response.msg {
                    send_event(Event::MsgReceived(msg));
                } else {
                    trace!("received unsupported msg, response.msg={:?}", response.msg);
                }
            }
            Err(nng::Error::TimedOut) => {
                let msg_port = *MSG_PORT.lock();
                if msg_port == 0 {
                    trace!("disabled receiving as user requested, now closing");
                    break;
                }
            }
            Err(e) => {
                error!("recv error! now closing, e={}", e);
                break;
            }
        }
    }
    socket.close();
    send_event(Event::MsgSocketDisconnected);
}

pub fn register_event_callback<F>(callback: F)
where
    F: FnMut(Event) + Send + 'static,
{
    *EVENT_CALLBACK.lock() = Some(Arc::new(Mutex::new(callback)));
}

pub fn unregister_event_callback() {
    *EVENT_CALLBACK.lock() = None;
}

pub fn init(port: u16, debug: bool, auto_clean: bool) -> Result<CleanupHandler> {
    trace!("init()");
    if loader::load_sdk_dll()? {
        send_event(Event::SdkDllLoaded);
    }
    let mut cmd_port = CMD_PORT.lock();
    if *cmd_port != 0 {
        return Err(anyhow!("wcf already inited"));
    }
    let init_sdk_result = loader::wx_init_sdk(debug, port as i32)?;
    if init_sdk_result != 0 {
        return Err(anyhow!("wcf init sdk failed, result={}", init_sdk_result));
    }
    *cmd_port = port;
    send_event(Event::SdkInited(port, debug));
    Ok(CleanupHandler { auto_clean })
}

pub fn uninit() {
    trace!("uninit()");
    let mut cmd_port = CMD_PORT.lock();
    if *cmd_port == 0 {
        return; // no need to uninit
    }

    disconnect_cmd_socket();
    let _ = disable_listen();

    match loader::wx_destroy_sdk() {
        Ok(0) => {}
        Ok(i) => warn!("wcf::uninit(), wx_destroy_sdk() returned result={}", i),
        Err(e) => warn!("wcf::uninit(), wx_destroy_sdk() returned error={:?}", e),
    }
    *cmd_port = 0;
    send_event(Event::SdkDestroyed);
}

pub fn connect_cmd_socket() -> Result<()> {
    let cmd_port = *CMD_PORT.lock();
    if cmd_port == 0 {
        return Err(anyhow!("wcf not inited"));
    }

    let mut cmd_socket = CMD_SOCKET.lock();
    if cmd_socket.is_some() {
        return Err(anyhow!("cmd_socket already connected"));
    }
    *cmd_socket = Some(connect_socket(cmd_port)?);
    send_event(Event::CmdSocketConnected);
    Ok(())
}

pub fn disconnect_cmd_socket() {
    let cmd_socket_disconnect = CMD_SOCKET.lock().take().is_some();
    if cmd_socket_disconnect {
        send_event(Event::CmdSocketDisconnected);
    }
}

pub fn is_login() -> Result<bool> {
    let response = run_cmd(proto::Functions::FuncIsLogin.into(), None)?;
    Ok(get_response_status_as_bool(&response))
}

pub fn get_self_wx_id() -> Result<Option<String>> {
    let response = run_cmd(proto::Functions::FuncGetSelfWxid.into(), None)?;
    match response.msg {
        Some(proto::response::Msg::Str(wx_id)) => Ok(Some(wx_id)),
        _ => Ok(None),
    }
}

pub fn get_user_info() -> Result<Option<UserInfo>> {
    let response = run_cmd(proto::Functions::FuncGetUserInfo.into(), None)?;
    match response.msg {
        Some(proto::response::Msg::Ui(user_info)) => Ok(Some(user_info.into())),
        _ => Ok(None),
    }
}

pub fn get_contacts() -> Result<Option<proto::RpcContacts>> {
    let response = run_cmd(proto::Functions::FuncGetContacts.into(), None)?;
    match response.msg {
        Some(proto::response::Msg::Contacts(contacts)) => Ok(Some(contacts)),
        _ => Ok(None),
    }
}

pub fn query_all_contact_info() -> Result<Vec<ContactInfo>> {
    let sql = "SELECT * FROM Contact LEFT JOIN ContactHeadImgUrl ON Contact.UserName = ContactHeadImgUrl.usrName";
    let rows = exec_db_query("MicroMsg.db".into(), sql.into())?;
    Ok(rows.into_iter().map(|row| row.into()).collect())
}

pub fn query_contact_info(wxid: String) -> Result<Option<ContactInfo>> {
    let sql = format!(
        "SELECT * FROM Contact \
        LEFT JOIN ContactHeadImgUrl \
        ON Contact.UserName = ContactHeadImgUrl.usrName \
        WHERE Contact.UserName = \"{}\"",
        wxid
    );
    let rows = exec_db_query("MicroMsg.db".into(), sql)?;
    Ok(rows.into_iter().next().map(|row| row.into()))
}

pub fn query_chat_room_info(wxid: String) -> Result<Option<ChatRoom>> {
    let sql = format!(
        "SELECT ChatRoom.ChatRoomName AS ChatRoomName, \
        ChatRoom.RoomData AS RoomData, \
        ContactHeadImgUrl.smallHeadImgUrl AS smallHeadImgUrl, \
        ChatRoomInfo.Announcement AS Announcement \
        FROM ChatRoom \
        LEFT JOIN ContactHeadImgUrl \
        ON ChatRoom.ChatRoomName = ContactHeadImgUrl.usrName \
        LEFT JOIN ChatRoomInfo \
        ON ChatRoom.ChatRoomName = ChatRoomInfo.ChatRoomName \
        WHERE ChatRoom.ChatRoomName = \"{}\"",
        wxid
    );
    let rows = exec_db_query("MicroMsg.db".into(), sql)?;
    Ok(rows.into_iter().next().map(|row| row.into()))
}

pub fn get_db_names() -> Result<Vec<String>> {
    let response = run_cmd(proto::Functions::FuncGetDbNames.into(), None)?;
    match response.msg {
        Some(proto::response::Msg::Dbs(dbs)) => Ok(dbs.names),
        _ => Ok(vec![]),
    }
}

pub fn get_db_tables(db: String) -> Result<Vec<proto::DbTable>> {
    let msg = Some(proto::request::Msg::Str(db));
    let response = run_cmd(proto::Functions::FuncGetDbTables.into(), msg)?;
    match response.msg {
        Some(proto::response::Msg::Tables(tables)) => Ok(tables.tables),
        _ => Ok(vec![]),
    }
}

pub fn exec_db_query(db: String, sql: String) -> Result<Vec<proto::DbRow>> {
    let db_query_msg = proto::DbQuery { db, sql };
    let msg = Some(proto::request::Msg::Query(db_query_msg));
    let response = run_cmd(proto::Functions::FuncExecDbQuery.into(), msg)?;
    match response.msg {
        Some(proto::response::Msg::Rows(rows)) => Ok(rows.rows),
        _ => Ok(vec![]),
    }
}

/**
 * @param msg:      消息内容（如果是 @ 消息则需要有跟 @ 的人数量相同的 @）
 * @param receiver: 消息接收人，私聊为 wxid（wxid_xxxxxxxxxxxxxx），群聊为
 *                  roomid（xxxxxxxxxx@chatroom）
 * @param aters:    群聊时要 @ 的人（私聊时为空字符串），多个用逗号分隔。@所有人 用
 *                  notify@all（必须是群主或者管理员才有权限）
 * @return int
 * @Description 发送文本消息
 * @author Changhua
 * @example sendText(" Hello @ 某人1 @ 某人2 ", " xxxxxxxx @ chatroom ",
 * "wxid_xxxxxxxxxxxxx1,wxid_xxxxxxxxxxxxx2");
 */
pub fn send_text(msg: String, receiver: String, aters: String) -> Result<bool> {
    let text_msg = proto::TextMsg { msg, receiver, aters };
    let msg = Some(proto::request::Msg::Txt(text_msg));
    let response = run_cmd(proto::Functions::FuncSendTxt.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

pub fn send_image(path: PathBuf, receiver: String) -> Result<bool> {
    let path_msg = proto::PathMsg { path: path.into_os_string().into_string().unwrap_or_default(), receiver };
    let msg = Some(proto::request::Msg::File(path_msg));
    let response = run_cmd(proto::Functions::FuncSendImg.into(), msg)?;
    Ok(response.msg.is_some())
}

pub fn send_file(path: PathBuf, receiver: String) -> Result<bool> {
    let path_msg = proto::PathMsg { path: path.into_os_string().into_string().unwrap_or_default(), receiver };
    let msg = Some(proto::request::Msg::File(path_msg));
    let response = run_cmd(proto::Functions::FuncSendFile.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

pub fn send_xml(xml: String, path: PathBuf, receiver: String, xml_type: i32) -> Result<bool> {
    let xml_msg = proto::XmlMsg {
        content: xml,
        path: path.into_os_string().into_string().unwrap_or_default(),
        receiver,
        r#type: xml_type,
    };
    let msg = Some(proto::request::Msg::Xml(xml_msg));
    let response = run_cmd(proto::Functions::FuncSendXml.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

pub fn send_emotion(path: PathBuf, receiver: String) -> Result<bool> {
    let path_msg = proto::PathMsg { path: path.into_os_string().into_string().unwrap_or_default(), receiver };
    let msg = Some(proto::request::Msg::File(path_msg));
    let response = run_cmd(proto::Functions::FuncSendEmotion.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

pub fn enable_listen() -> Result<()> {
    let msg_port = {
        let mut msg_port = MSG_PORT.lock();
        if *msg_port == 0 {
            // only send command when msg_port not set
            let cmd_port = *CMD_PORT.lock();
            if cmd_port == 0 {
                return Err(anyhow!("wcf not inited"));
            }
            let msg = Some(proto::request::Msg::Flag(true));
            let response = run_cmd(proto::Functions::FuncEnableRecvTxt.into(), msg)?;
            if response.msg.is_none() {
                return Err(anyhow!("failed to enable remote listen service"));
            }
            *msg_port = cmd_port + 1;
        }
        *msg_port
    };
    // start recv msg thread
    std::thread::spawn(move || recv_msg_thread(msg_port));
    Ok(())
}

pub fn disable_listen() -> Result<bool> {
    let mut msg_port = MSG_PORT.lock();
    if *msg_port == 0 {
        return Ok(false); // no need to disable
    }

    let response = run_cmd(proto::Functions::FuncDisableRecvTxt.into(), None)?;
    match response.msg {
        Some(_) => {
            *msg_port = 0;
            Ok(true)
        }
        None => Err(anyhow!("failed to disable recv, None returned from remote side")),
    }
}

/**
 * 获取消息类型
 * {"47": "石头剪刀布 | 表情图片", "62": "小视频", "43": "视频", "1": "文字", "10002": "撤回消息", "40": "POSSIBLEFRIEND_MSG", "10000": "红包、系统消息", "37": "好友确认", "48": "位置", "42": "名片", "49": "共享实时位置、文件、转账、链接", "3": "图片", "34": "语音", "9999": "SYSNOTICE", "52": "VOIPNOTIFY", "53": "VOIPINVITE", "51": "微信初始化", "50": "VOIPMSG"}
 */
pub fn get_msg_types() -> Result<HashMap<i32, String>> {
    let response = run_cmd(proto::Functions::FuncGetMsgTypes.into(), None)?;
    match response.msg {
        Some(proto::response::Msg::Types(msg_types)) => Ok(msg_types.types),
        _ => Ok(HashMap::default()),
    }
}

pub fn accept_new_friend(v3: String, v4: String, scene: i32) -> Result<bool> {
    let msg = Some(proto::request::Msg::V(proto::Verification { v3, v4, scene }));
    let response = run_cmd(proto::Functions::FuncAcceptFriend.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

/* 添加群成员 */
pub fn add_chatroom_members(roomid: String, wxids: String) -> Result<bool> {
    let msg = Some(proto::request::Msg::M(proto::MemberMgmt { roomid, wxids }));
    let response = run_cmd(proto::Functions::FuncAddRoomMembers.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

/* 邀请群成员 */
pub fn inv_chatroom_members(roomid: String, wxids: String) -> Result<bool> {
    let msg = Some(proto::request::Msg::M(proto::MemberMgmt { roomid, wxids }));
    let response = run_cmd(proto::Functions::FuncInvRoomMembers.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

/* 删除群成员 */
pub fn del_chatroom_members(roomid: String, wxids: String) -> Result<bool> {
    let msg = Some(proto::request::Msg::M(proto::MemberMgmt { roomid, wxids }));
    let response = run_cmd(proto::Functions::FuncDelRoomMembers.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

pub fn decrypt_image(src: String, dst: String) -> Result<bool> {
    let msg = Some(proto::request::Msg::Dec(proto::DecPath { src, dst }));
    let response = run_cmd(proto::Functions::FuncDecryptImage.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

pub fn recv_transfer(wxid: String, transferid: String, transcationid: String) -> Result<bool> {
    let (tfid, taid) = (transferid, transcationid);
    let msg = Some(proto::request::Msg::Tf(proto::Transfer { wxid, tfid, taid }));
    let response = run_cmd(proto::Functions::FuncRecvTransfer.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

/** 刷新朋友圈 */
pub fn refresh_pyq(id: u64) -> Result<bool> {
    let msg = Some(proto::request::Msg::Ui64(id));
    let response = run_cmd(proto::Functions::FuncRefreshPyq.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

/** 保存附件 */
pub fn attach_msg(id: u64, thumb: String, extra: String) -> Result<bool> {
    let msg = Some(proto::request::Msg::Att(proto::AttachMsg { id, thumb, extra }));
    let response = run_cmd(proto::Functions::FuncDownloadAttach.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

/** 获取语音 */
pub fn get_audio_msg(id: u64, dir: String) -> Result<bool> {
    let msg = Some(proto::request::Msg::Am(proto::AudioMsg { id, dir }));
    let response = run_cmd(proto::Functions::FuncGetAudioMsg.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

/** 发送富文本 */
pub fn send_rich_text(richtext: proto::RichText) -> Result<bool> {
    let msg = Some(proto::request::Msg::Rt(richtext));
    let response = run_cmd(proto::Functions::FuncSendRichTxt.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

/** 发送拍一拍 */
pub fn send_pat_msg(roomid: String, wxid: String) -> Result<bool> {
    let msg = Some(proto::request::Msg::Pm(proto::PatMsg { roomid, wxid }));
    let response = run_cmd(proto::Functions::FuncSendPatMsg.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}

/** OCR */
pub fn exec_ocr(path: PathBuf) -> Result<Option<proto::OcrMsg>> {
    let path_str = path.into_os_string().into_string().map_err(|_| anyhow!("invalid path"))?;
    let msg = Some(proto::request::Msg::Str(path_str));
    let response = run_cmd(proto::Functions::FuncExecOcr.into(), msg)?;
    match response.msg {
        Some(proto::response::Msg::Ocr(msg)) => Ok(Some(msg)),
        _ => Ok(None),
    }
}

/** 转发消息 */
pub fn forward_msg(id: u64, receiver: String) -> Result<bool> {
    let msg = Some(proto::request::Msg::Fm(proto::ForwardMsg { id, receiver }));
    let response = run_cmd(proto::Functions::FuncForwardMsg.into(), msg)?;
    Ok(get_response_status_as_bool(&response))
}
