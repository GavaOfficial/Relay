#include <Geode/Geode.hpp>
#include <Geode/modify/GJBaseGameLayer.hpp>
#include <Geode/modify/PlayLayer.hpp>

#include <chrono>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <unordered_set>

using namespace geode::prelude;

namespace {

double nowMs() {
	using namespace std::chrono;
	return static_cast<double>(duration_cast<milliseconds>(system_clock::now().time_since_epoch()).count());
}

std::filesystem::path eventsFile() {
	char const* appdata = std::getenv("APPDATA");
	if (!appdata) return {};
	return std::filesystem::path(appdata) / "Relay" / "gd-events.jsonl";
}

void emit(matjson::Value data) {
	auto path = eventsFile();
	if (path.empty()) return;
	std::error_code ec;
	std::filesystem::create_directories(path.parent_path(), ec);
	data["engine"] = "gd";
	data["sent_at"] = nowMs();
	data["game_dir"] = geode::dirs::getGameDir().string();
	std::ofstream out(path, std::ios::app | std::ios::binary);
	if (out) out << data.dump(matjson::NO_INDENTATION) << "\n";
}

std::string section(GJGameLevel* level, bool platformer) {
	if (platformer) return "Platformer";
	if (level->m_levelType == GJLevelType::Main) return "Livelli principali";
	return "Livelli online";
}

matjson::Value levelInfo(GJGameLevel* level, bool platformer) {
	auto info = matjson::Value::object();
	info["id"] = static_cast<int>(level->m_levelID);
	info["name"] = std::string(level->m_levelName);
	info["creator"] = std::string(level->m_creatorName);
	info["type"] = static_cast<int>(level->m_levelType);
	info["difficulty"] = static_cast<int>(level->m_difficulty);
	info["average_difficulty"] = level->getAverageDifficulty();
	info["demon"] = static_cast<int>(level->m_demon) != 0;
	info["demon_difficulty"] = level->m_demonDifficulty;
	info["auto"] = level->m_autoLevel;
	info["stars"] = static_cast<int>(level->m_stars);
	info["featured"] = level->m_featured;
	info["epic"] = level->m_isEpic;
	info["coins"] = level->m_coins;
	info["coins_verified"] = static_cast<int>(level->m_coinsVerified) != 0;
	info["length"] = level->m_levelLength;
	info["platformer"] = platformer;
	info["song_id"] = level->m_songID;
	info["audio_track"] = level->m_audioTrack;
	info["attempts"] = static_cast<int>(level->m_attempts);
	info["jumps"] = static_cast<int>(level->m_jumps);
	info["best_percent"] = static_cast<int>(level->m_normalPercent);
	info["best_time"] = level->m_bestTime;
	return info;
}

matjson::Value profile() {
	auto p = matjson::Value::object();
	auto acc = GJAccountManager::sharedState();
	p["username"] = std::string(acc->m_username);
	p["account_id"] = acc->m_accountID;
	auto stats = GameStatsManager::sharedState();
	auto stat = [&](char const* key) { return stats->getStat(key); };
	p["stars"] = stat("6");
	p["moons"] = stat("28");
	p["demons"] = stat("5");
	p["secret_coins"] = stat("8");
	p["user_coins"] = stat("12");
	p["diamonds"] = stat("13");
	p["jumps"] = stat("1");
	p["attempts"] = stat("2");
	auto gm = GameManager::sharedState();
	p["icon"] = gm->getPlayerFrame();
	p["color1"] = gm->getPlayerColor();
	p["color2"] = gm->getPlayerColor2();
	p["glow"] = gm->m_playerGlow;
	return p;
}

std::unordered_set<GameObject*> g_coins;

bool isCoin(GameObject* obj) {
	return obj && (obj->m_objectID == 142 || obj->m_objectID == 1329);
}

}

class $modify(RelayGameLayer, GJBaseGameLayer) {
	void destroyObject(GameObject* object) {
		if (isCoin(object) && typeinfo_cast<PlayLayer*>(this)) g_coins.insert(object);
		GJBaseGameLayer::destroyObject(object);
	}
};

class $modify(RelayPlayLayer, PlayLayer) {
	struct Fields {
		bool attemptOpen = false;
		bool ended = false;
		bool loaded = false;
		int attempt = 0;
	};

	bool valid() {
		auto speed = CCDirector::sharedDirector()->getScheduler()->getTimeScale();
		return !m_isPracticeMode && !m_isTestMode && m_startPosObject == nullptr
			&& m_level->m_levelType != GJLevelType::Editor && std::abs(speed - 1.f) < 0.001f;
	}

	matjson::Value base(char const* kind) {
		auto data = matjson::Value::object();
		data["kind"] = kind;
		data["song_name"] = std::string(m_level->m_levelName);
		data["song_id"] = std::to_string(static_cast<int>(m_level->m_levelID));
		data["difficulty"] = m_isPlatformer ? "platformer" : "classic";
		data["mod_folder"] = section(m_level, m_isPlatformer);
		data["valid"] = valid();
		return data;
	}

	void announceLevel() {
		auto data = base("song_load");
		auto extra = matjson::Value::object();
		extra["level"] = levelInfo(m_level, m_isPlatformer);
		extra["profile"] = profile();
		data["data"] = extra;
		emit(data);
	}

	void resetLevel() {
		PlayLayer::resetLevel();
		g_coins.clear();
		auto f = m_fields.self();
		if (!f->loaded) {
			f->loaded = true;
			announceLevel();
		}
		f->attemptOpen = true;
		f->ended = false;
		f->attempt += 1;
		auto data = base("song_start");
		auto extra = matjson::Value::object();
		extra["attempt"] = f->attempt;
		data["data"] = extra;
		emit(data);
	}

	void destroyPlayer(PlayerObject* player, GameObject* object) {
		PlayLayer::destroyPlayer(player, object);
		auto f = m_fields.self();
		if (!f->attemptOpen || f->ended || !m_player1 || !m_player1->m_isDead) return;
		f->ended = true;
		f->attemptOpen = false;
		auto data = base(m_isPlatformer ? "song_abort" : "song_end");
		int percent = getCurrentPercentInt();
		auto extra = matjson::Value::object();
		extra["percent"] = percent;
		extra["completed"] = false;
		extra["coins"] = 0;
		extra["attempt"] = f->attempt;
		extra["time_ms"] = static_cast<int>(m_attemptTime * 1000.0);
		data["data"] = extra;
		data["score"] = percent * 1000;
		data["reason"] = "morte";
		emit(data);
	}

	void levelComplete() {
		auto f = m_fields.self();
		bool open = f->attemptOpen && !f->ended;
		int coins = std::max(static_cast<int>(g_coins.size()), countCollectedUserCoins());
		double seconds = m_gameState.m_levelTime > 0 ? m_gameState.m_levelTime : m_attemptTime;
		PlayLayer::levelComplete();
		if (!open) return;
		f->ended = true;
		f->attemptOpen = false;
		auto data = base("song_end");
		int timeMs = static_cast<int>(seconds * 1000.0);
		auto extra = matjson::Value::object();
		extra["percent"] = 100;
		extra["completed"] = true;
		extra["coins"] = coins;
		extra["attempt"] = f->attempt;
		extra["time_ms"] = timeMs;
		data["data"] = extra;
		data["score"] = m_isPlatformer ? (1000000000LL - timeMs) : (100 * 1000 + coins);
		emit(data);
	}

	void onQuit() {
		auto f = m_fields.self();
		if (f->attemptOpen && !f->ended) {
			auto data = base("song_abort");
			data["reason"] = "uscita dal livello";
			emit(data);
		}
		f->attemptOpen = false;
		PlayLayer::onQuit();
	}
};
