"""AstrBot knowledge-base storage with deferred opening of empty databases."""

from __future__ import annotations

import asyncio
import sqlite3
from contextlib import asynccontextmanager, closing
from pathlib import Path

from astrbot.core.knowledge_base import kb_mgr
from astrbot.core.knowledge_base.kb_db_sqlite import KBSQLiteDatabase


def _has_existing_bases(path: Path) -> bool:
    if not path.exists():
        return False
    try:
        with closing(sqlite3.connect(f"{path.as_uri()}?mode=ro", uri=True, timeout=0)) as db:
            return db.execute("SELECT 1 FROM knowledge_bases LIMIT 1").fetchone() is not None
    except sqlite3.Error:
        # Let AstrBot handle older schemas and any database errors as usual.
        return True


class LazyKBSQLiteDatabase(KBSQLiteDatabase):
    def __init__(self, path: str) -> None:
        super().__init__(path)
        self._init_lock = asyncio.Lock()
        self._initializing_task: asyncio.Task | None = None

    async def initialize(self) -> None:
        async with self._init_lock:
            if self.inited:
                return
            self._initializing_task = asyncio.current_task()
            ready = False
            try:
                await super().initialize()
                await super().migrate_to_v1()
                ready = True
            finally:
                self.inited = ready
                self._initializing_task = None

    @asynccontextmanager
    async def get_db(self):
        # The native migration opens its own session while holding the lock.
        if asyncio.current_task() is not self._initializing_task:
            await self.initialize()
        async with super().get_db() as session:
            yield session


class FishKnowledgeBaseManager(kb_mgr.KnowledgeBaseManager):
    async def initialize(self) -> None:
        if _has_existing_bases(kb_mgr.DB_PATH):
            await super().initialize()
            return

        self.kb_db = LazyKBSQLiteDatabase(kb_mgr.DB_PATH.as_posix())
        self.retrieval_manager = kb_mgr.RetrievalManager(
            sparse_retriever=kb_mgr.SparseRetriever(self.kb_db),
            rank_fusion=kb_mgr.RankFusion(self.kb_db),
            kb_db=self.kb_db,
        )
