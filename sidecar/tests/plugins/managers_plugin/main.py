import asyncio
import sys
from datetime import datetime, timedelta, timezone

from astrbot.api.event import AstrMessageEvent, MessageChain, filter
from astrbot.api.star import Context, Star
from astrbot.core.provider.provider import EmbeddingProvider


class TestEmbedding(EmbeddingProvider):
    def get_dim(self):
        return 2

    async def get_embedding(self, text):
        return [1.0, 0.0]

    async def get_embeddings(self, texts):
        return [[1.0, 0.0] for _ in texts]


class ManagersPlugin(Star):
    def __init__(self, context: Context):
        super().__init__(context)
        context.register_provider(TestEmbedding({"id": "fixture-embedding"}, {}))
        assert any(profile["id"] == "default" for profile in context.astrbot_config_mgr.get_conf_list())

    @filter.command("open_empty_kb")
    async def open_empty_kb(self, event: AstrMessageEvent):
        db = self.context.kb_manager.kb_db
        before = db.inited
        count, bases = await asyncio.gather(db.count_kbs(), db.list_kbs())
        yield event.plain_result(f"{before}|{db.inited}|{count}|{len(bases)}")

    @filter.command("faiss_imported")
    async def faiss_imported(self, event: AstrMessageEvent):
        yield event.plain_result(str("faiss" in sys.modules))

    @filter.command("populate")
    async def populate(self, event: AstrMessageEvent):
        await self.context.persona_manager.create_persona("fish-teacher", "Talk about fish.")
        configs = self.context.astrbot_config_mgr
        conf_id = await configs.create_conf(name="Fish profile")
        await configs.ucr.update_route(event.unified_msg_origin, conf_id)
        kb = await self.context.kb_manager.create_kb("Fish notes", embedding_provider_id="fixture-embedding")
        await kb.upload_document("notes.txt", b"Fish sleep in the pond.", "txt")
        yield event.plain_result(conf_id)

    @filter.command("managers")
    async def managers(self, event: AstrMessageEvent):
        persona = await self.context.persona_manager.get_persona("fish-teacher")
        config = self.context.astrbot_config_mgr.get_conf_info(event.unified_msg_origin)
        assert self.context.get_config(event.unified_msg_origin) is self.context.astrbot_config_mgr.confs[config["id"]]
        notes = await self.context.kb_manager.retrieve("Where do fish sleep?", ["Fish notes"])
        yield event.plain_result(f"{persona.system_prompt}|{config['name']}|{notes['results'][0]['content']}")

    @filter.command("compare_txt")
    async def compare_txt(self, event: AstrMessageEvent):
        from astrbot.core.knowledge_base.kb_helper import select_parser

        samples = [
            b"first  \r\nsecond\r\n\r\n\r\nthird",
            "池塘里的鱼。\n".encode("utf-8-sig"),
            "这是一个用来测试编码的知识库文档。".encode("gb18030"),
            "Wide text.\r\nAnother line.".encode("utf-16"),
            b"<h1>HTML as plain text</h1>\n",
            b"",
        ]
        plain_parser = await select_parser(".txt")
        actual = [await plain_parser.parse(data, "notes.TXT") for data in samples]
        assert "markitdown_no_magika" not in sys.modules, "plain text loaded all document converters"

        from astrbot.core.knowledge_base.parsers.markitdown_parser import MarkitdownParser

        native = MarkitdownParser()
        expected = [await native.parse(data, "notes.TXT") for data in samples]
        assert [(result.text, result.media) for result in actual] == [
            (result.text, result.media) for result in expected
        ]
        fallback = await select_parser(".docx")
        assert isinstance(fallback, MarkitdownParser)
        yield event.plain_result("text matches native parser")

    @filter.command("cron_basic")
    async def cron_basic(self, event: AstrMessageEvent):
        async def deliver():
            await self.context.send_message(event.unified_msg_origin, MessageChain().message("scheduled fish"))

        cron = self.context.cron_manager
        job = await cron.add_basic_job(name="fish fixture", cron_expression="* * * * *", handler=deliver)
        assert cron.get_next_run_time(job.job_id)
        await cron.run_job_now(job.job_id)
        assert next(j for j in await cron.list_jobs() if j.job_id == job.job_id).status == "completed"
        await cron.delete_job(job.job_id)
        yield event.plain_result("cron completed")


    @filter.command("cron_once")
    async def cron_once(self, event: AstrMessageEvent):
        job = await self.context.cron_manager.add_active_job(
            name="fish reminder", cron_expression=None, run_once=True,
            run_at=datetime.now(timezone.utc) + timedelta(seconds=5),
            payload={"session": event.unified_msg_origin, "note": "Send a fish reminder."},
        )
        yield event.plain_result(job.job_id)
