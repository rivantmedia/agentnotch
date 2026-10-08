-- The copies of one Claude session that different people synced (same account and session id):
-- the website counts one of them, and finds the others through this index, so the check stays
-- one lookup per session on an account shared through a pool.

-- CreateIndex
CREATE INDEX "Session_accountKey_sessionId_idx" ON "Session"("accountKey", "sessionId");
