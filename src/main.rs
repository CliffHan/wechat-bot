use anyhow::Result;
use std::time::Duration;

mod wechatferry;

fn main() -> Result<()> {
    env_logger::init();

    // 注册回调函数，参考 wcf::Event
    wechatferry::register_event_callback(|event| {
        println!("received event: {:?}", event);
        // 注意，回调函数有可能是当前线程回调，也有可能是接收线程回调，不能在此函数中做复杂操作，否则可能死锁。
        // 如果希望通过回调执行复杂操作，请使用 channel 通知其他线程执行。
    });
    // auto_clean 为 true 时，返回值必须保留，否则会被自动清理
    let _cleanup = wechatferry::init(10086, true, true)?;
    // 显式连接 command socket 后才可以调用下列测试函数
    wechatferry::connect_cmd_socket()?;

    // 测试所需变量，按实际情况设置
    // let wxid = ""; // your wxid
    // let chatroom_name = ""; // your chatroom name
    // let db_name = "MicroMsg.db";
    // let image_file = ""; // your image file here

    // 测试函数，按需启用
    println!("is_login={}", wechatferry::is_login()?);
    // println!("get_self_wx_id={:?}", wcf::get_self_wx_id()?);
    // println!("get_user_info={:?}", wcf::get_user_info()?);
    // println!("get_contacts={:?}", wcf::get_contacts()?);
    // println!("db_names={:?}", wcf::get_db_names()?);
    // println!("db_tables={:?}", wcf::get_db_tables(db_name.into())?);
    // println!("query_all_contact_info={:?}", wcf::query_all_contact_info()?);
    // println!("query_contact_info={:?}", wcf::query_contact_info(wxid.into())?);
    // println!("query_chat_room_info={:?}", wcf::query_chat_room_info(chatroom_name.into())?);
    // let sql = "SELECT ChatRoomName FROM ChatRoom";
    // println!("exec_db_query={:?}", wcf::exec_db_query("MicroMsg.db".into(), sql.into())?);

    // let send_result = wcf::send_text("Are you ok?".into(), wxid.into(), "".into())?;
    // println!("send_result={}", send_result);
    // let send_result = wcf::send_image(image_file.into(), wxid.into())?;
    // println!("send_result={}", send_result);

    wechatferry::enable_listen()?;
    println!("waiting 60s to receive msg...");
    std::thread::sleep(Duration::from_secs(60)); // check received msg in callback
    wechatferry::disable_listen()?;

    // println!("get_msg_types={:?}", wcf::get_msg_types()?);

    // Note: these functions are not tested, but I think they should work
    // send_file, send_xml, send_emotion
    // accept_new_friend, add_chatroom_members, inv_chatroom_members, del_chatroom_members
    // decrypt_image, recv_transfer, refresh_pyq, attach_msg, get_audio_msg
    // send_rich_text, send_pat_msg, exec_ocr, forward_msg

    // wcf::uninit(); // auto_clean 为 false 时，需要显式调用 uninit()
    println!("waiting 5s to ensure msg socket closed...");
    std::thread::sleep(Duration::from_secs(5)); // msg socket 可能尚未关闭，等 5s
    Ok(())
}
