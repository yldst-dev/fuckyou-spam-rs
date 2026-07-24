use teloxide::{types::BotCommand, utils::command::BotCommands};

#[derive(BotCommands, Clone)]
#[command(rename_rule = "snake_case", description = "사용 가능한 명령어:")]
pub(super) enum GeneralCommand {
    #[command(description = "봇 소개 및 시작")]
    Start,
    #[command(description = "도움말")]
    Help,
    #[command(description = "봇 상태 확인")]
    Status,
    #[command(description = "현재 채팅 ID 확인")]
    Chatid,
    #[command(description = "응답 속도 측정")]
    Ping,
}

pub(super) fn admin_command_list() -> Vec<BotCommand> {
    let mut commands = GeneralCommand::bot_commands();
    commands.extend(vec![
        BotCommand::new("whitelist_add", "그룹을 화이트리스트에 추가"),
        BotCommand::new("whitelist_remove", "화이트리스트에서 제거"),
        BotCommand::new("whitelist_list", "화이트리스트 목록"),
        BotCommand::new("sync_commands", "봇 명령어 동기화"),
    ]);
    commands
}
