//! SQLite store (`data/pumpstudio.db`): profiles, settings per profile, pumps, ingredients and an index
//! of the application log (the day file in `logs/` stays the primary log). Every change is written
//! immediately; there is no "save on exit".
//!
//! Schema version 1:
//! - `schema_version(version)`; `meta(key, value)` for app-wide values (active profile, imports done).
//! - `profile(id, name, kind)`: VSD and Keramisch.
//! - `settings(profile_id, key, value)`: JSON per key (`machine`, `bench`, `python`, `repo`).
//! - `ingredient(id, profile_id, name, density, notes)`.
//! - `pump(id, profile_id, sort, name, head, node, motor, ingredient_id, enabled, position, pump_type, steps_rev, notes)`.
//! - `log(id, time, level, area, text)` with indexes on time and level/area.
//!
//! Schema version 2 (iteration 2) adds calibrations (see `calib.rs`):
//! - `calibration(id, profile_id, pump_id, pump_name, date_ms, kind, simulation, settings, g_per_rev, g_per_stroke,
//!   spread, repaired, status actief/archief/afgebroken, note, source, check_requested, check_measured, check_error_pct, data)`;
//! - `calibration_point(id, calibration_id, phase, run, idx, position_steps, grams, repaired)`.
//!
//! Schema version 5 (iteration 5) adds production (see `production.rs`): `prod_order(id, profile_id, recipe_version_id,
//! name, batch, priority, count, created_ms, status, note)`, per pot `pot_run.queue_idx`, `pot_nr`, `spot` (barcode unique,
//! `head_state` = state per head as JSON), `line_state` (the line model, one row) and `prod_event` (scan/transport/hold history).
//!
//! Schema version 4 (iteration 4) adds ingredients ↔ pumps, recipes with versions and pot runs (see `recipe.rs`):
//! - `ingredient` + `tolerance_pct`, `head_pref`; `ingredient_pump(ingredient_id, pump_id, preferred)` kept in step with
//!   `pump.ingredient_id` by triggers (the Pompen page edits the pump; one truth);
//! - `recipe(id, profile_id, name, description, pot_g, tolerance_pct, current_version, archived, created_ms)`,
//!   `recipe_version(id, recipe_id, version, created_ms, note, description, pot_g, tolerance_pct, pot_height_cm)`,
//!   `recipe_step(id, version_id, idx, topup, note)`, `recipe_item(id, step_id, idx, ingredient_id, grams, tolerance_pct, head)`;
//! - `pot_run(id, profile_id, recipe_version_id, simulation, mode, status, started_ms, ended_ms, barcode, order_id, note, head_state)`,
//!   `pot_item(id, pot_run_id, step_idx, item_idx, ingredient_id, ingredient, pump, head, requested_g, dosed_g, attributed,
//!   topped_up, deviation_pct, tolerance_pct, status, calibration_id, steps, started_ms, ended_ms, note)`.
//!
//! Schema version 3 (iteration 3) adds pump tests (see `testdb.rs`):
//! - `test_run(id, profile_id, pump_id, pump_name, simulation, calibration_id, mode, plan JSON incl. seed, tolerance_pct,
//!   status loopt/klaar/gestopt/fout/onderbroken, started_ms, ended_ms, note, source)`;
//! - `test_dose(id, run_id, nr, requested_g, dosed_g, correction_pct, target_g, steps, settings JSON, duration_s, status, error,
//!   excluded, exclude_reason, time_ms)`.
use crate::{applog::Line,machine::{default_pumps,Head,MachineConfig,VsdPump},model::{Config,Result}};
use rusqlite::{params,Connection,OptionalExtension};
// Every write transaction is IMMEDIATE: it takes the write lock at its start, so a concurrent writer
// (log index, CAN worker, UI) makes it wait (busy timeout) instead of failing with a stale snapshot.
use serde::{de::DeserializeOwned,Serialize};
use std::path::{Path,PathBuf};

pub const SCHEMA_VERSION:i64=6;
/// Version 2: calibrations with their measuring points (positions in steps).
const V2:&str="
    CREATE TABLE IF NOT EXISTS calibration(id INTEGER PRIMARY KEY, profile_id INTEGER NOT NULL REFERENCES profile(id),
        pump_id INTEGER REFERENCES pump(id), pump_name TEXT NOT NULL, date_ms INTEGER NOT NULL, kind TEXT NOT NULL,
        simulation INTEGER NOT NULL, settings TEXT NOT NULL DEFAULT '{}', g_per_rev REAL, g_per_stroke REAL, spread REAL,
        repaired INTEGER NOT NULL DEFAULT 0, status TEXT NOT NULL, note TEXT NOT NULL DEFAULT '', source TEXT NOT NULL DEFAULT '',
        check_requested REAL, check_measured REAL, check_error_pct REAL, data TEXT NOT NULL DEFAULT '{}');
    CREATE INDEX IF NOT EXISTS calibration_pump ON calibration(profile_id,pump_name,date_ms);
    CREATE TABLE IF NOT EXISTS calibration_point(id INTEGER PRIMARY KEY, calibration_id INTEGER NOT NULL REFERENCES calibration(id) ON DELETE CASCADE,
        phase INTEGER NOT NULL, run INTEGER NOT NULL, idx INTEGER NOT NULL, position_steps INTEGER NOT NULL, grams REAL NOT NULL, repaired INTEGER NOT NULL DEFAULT 0);
    CREATE INDEX IF NOT EXISTS calibration_point_cal ON calibration_point(calibration_id);";
/// Version 6: indexes for Analyse (period filters on pots and test doses).
const V6:&str="
    CREATE INDEX IF NOT EXISTS pot_run_started ON pot_run(profile_id,started_ms);
    CREATE INDEX IF NOT EXISTS test_dose_time ON test_dose(time_ms);";
/// Version 5: production orders, queue position and line spot per pot, the line model, production events.
const V5:&str="
    CREATE TABLE IF NOT EXISTS prod_order(id INTEGER PRIMARY KEY, profile_id INTEGER NOT NULL REFERENCES profile(id),
        recipe_version_id INTEGER NOT NULL REFERENCES recipe_version(id), name TEXT NOT NULL DEFAULT '', batch TEXT NOT NULL DEFAULT '',
        priority INTEGER NOT NULL DEFAULT 0, count INTEGER NOT NULL, created_ms INTEGER NOT NULL, status TEXT NOT NULL DEFAULT 'open', note TEXT NOT NULL DEFAULT '');
    ALTER TABLE pot_run ADD COLUMN queue_idx INTEGER;
    ALTER TABLE pot_run ADD COLUMN pot_nr INTEGER;
    ALTER TABLE pot_run ADD COLUMN spot TEXT;
    CREATE INDEX IF NOT EXISTS pot_run_status ON pot_run(mode,status,queue_idx);
    CREATE TABLE IF NOT EXISTS line_state(id INTEGER PRIMARY KEY CHECK(id=1), state TEXT NOT NULL, slots TEXT NOT NULL, message TEXT NOT NULL DEFAULT '', updated_ms INTEGER NOT NULL);
    CREATE TABLE IF NOT EXISTS prod_event(id INTEGER PRIMARY KEY, ms INTEGER NOT NULL, pot_run_id INTEGER, kind TEXT NOT NULL, text TEXT NOT NULL);
    CREATE INDEX IF NOT EXISTS prod_event_pot ON prod_event(pot_run_id);";
/// Version 4: ingredients linked to pumps, recipes (versions, steps, items) and pot runs.
const V4:&str="
    ALTER TABLE ingredient ADD COLUMN tolerance_pct REAL;
    ALTER TABLE ingredient ADD COLUMN head_pref TEXT;
    CREATE TABLE IF NOT EXISTS ingredient_pump(id INTEGER PRIMARY KEY, ingredient_id INTEGER NOT NULL REFERENCES ingredient(id),
        pump_id INTEGER NOT NULL UNIQUE REFERENCES pump(id), preferred INTEGER NOT NULL DEFAULT 0);
    INSERT OR IGNORE INTO ingredient_pump(ingredient_id,pump_id) SELECT ingredient_id,id FROM pump WHERE ingredient_id IS NOT NULL;
    CREATE TRIGGER IF NOT EXISTS pump_ing_ins AFTER INSERT ON pump WHEN NEW.ingredient_id IS NOT NULL BEGIN
        INSERT OR REPLACE INTO ingredient_pump(ingredient_id,pump_id,preferred) VALUES(NEW.ingredient_id,NEW.id,0); END;
    CREATE TRIGGER IF NOT EXISTS pump_ing_upd AFTER UPDATE OF ingredient_id ON pump BEGIN
        DELETE FROM ingredient_pump WHERE pump_id=NEW.id AND (NEW.ingredient_id IS NULL OR ingredient_id<>NEW.ingredient_id);
        INSERT OR IGNORE INTO ingredient_pump(ingredient_id,pump_id,preferred) SELECT NEW.ingredient_id,NEW.id,0 WHERE NEW.ingredient_id IS NOT NULL; END;
    CREATE TRIGGER IF NOT EXISTS pump_ing_del BEFORE DELETE ON pump BEGIN DELETE FROM ingredient_pump WHERE pump_id=OLD.id; END;
    CREATE TABLE IF NOT EXISTS recipe(id INTEGER PRIMARY KEY, profile_id INTEGER NOT NULL REFERENCES profile(id), name TEXT NOT NULL,
        description TEXT NOT NULL DEFAULT '', pot_g REAL, tolerance_pct REAL NOT NULL DEFAULT 1, current_version INTEGER, archived INTEGER NOT NULL DEFAULT 0,
        created_ms INTEGER NOT NULL, UNIQUE(profile_id,name));
    CREATE TABLE IF NOT EXISTS recipe_version(id INTEGER PRIMARY KEY, recipe_id INTEGER NOT NULL REFERENCES recipe(id), version INTEGER NOT NULL,
        created_ms INTEGER NOT NULL, note TEXT NOT NULL DEFAULT '', description TEXT NOT NULL DEFAULT '', pot_g REAL, tolerance_pct REAL NOT NULL DEFAULT 1,
        pot_height_cm REAL, UNIQUE(recipe_id,version));
    CREATE TABLE IF NOT EXISTS recipe_step(id INTEGER PRIMARY KEY, version_id INTEGER NOT NULL REFERENCES recipe_version(id) ON DELETE CASCADE,
        idx INTEGER NOT NULL, topup INTEGER NOT NULL DEFAULT 0, note TEXT NOT NULL DEFAULT '');
    CREATE TABLE IF NOT EXISTS recipe_item(id INTEGER PRIMARY KEY, step_id INTEGER NOT NULL REFERENCES recipe_step(id) ON DELETE CASCADE,
        idx INTEGER NOT NULL, ingredient_id INTEGER NOT NULL REFERENCES ingredient(id), grams REAL NOT NULL, tolerance_pct REAL, head TEXT);
    CREATE TABLE IF NOT EXISTS pot_run(id INTEGER PRIMARY KEY, profile_id INTEGER NOT NULL REFERENCES profile(id),
        recipe_version_id INTEGER NOT NULL REFERENCES recipe_version(id), simulation INTEGER NOT NULL, mode TEXT NOT NULL DEFAULT 'handmatig',
        status TEXT NOT NULL, started_ms INTEGER NOT NULL, ended_ms INTEGER, barcode TEXT UNIQUE, order_id INTEGER, note TEXT NOT NULL DEFAULT '', head_state TEXT);
    CREATE INDEX IF NOT EXISTS pot_run_version ON pot_run(recipe_version_id);
    CREATE TABLE IF NOT EXISTS pot_item(id INTEGER PRIMARY KEY, pot_run_id INTEGER NOT NULL REFERENCES pot_run(id) ON DELETE CASCADE,
        step_idx INTEGER NOT NULL, item_idx INTEGER NOT NULL, ingredient_id INTEGER, ingredient TEXT NOT NULL, pump TEXT NOT NULL, head TEXT NOT NULL,
        requested_g REAL NOT NULL, dosed_g REAL, attributed INTEGER NOT NULL DEFAULT 0, topped_up INTEGER NOT NULL DEFAULT 0, deviation_pct REAL,
        tolerance_pct REAL NOT NULL, status TEXT NOT NULL, calibration_id INTEGER, steps INTEGER, started_ms INTEGER, ended_ms INTEGER, note TEXT NOT NULL DEFAULT '');
    CREATE INDEX IF NOT EXISTS pot_item_run ON pot_item(pot_run_id,step_idx,item_idx);";
/// Version 3: pump tests and their doses.
const V3:&str="
    CREATE TABLE IF NOT EXISTS test_run(id INTEGER PRIMARY KEY, profile_id INTEGER NOT NULL REFERENCES profile(id), pump_id INTEGER REFERENCES pump(id),
        pump_name TEXT NOT NULL, simulation INTEGER NOT NULL, calibration_id INTEGER REFERENCES calibration(id), mode TEXT NOT NULL, plan TEXT NOT NULL DEFAULT '{}',
        tolerance_pct REAL NOT NULL, status TEXT NOT NULL, started_ms INTEGER NOT NULL, ended_ms INTEGER, note TEXT NOT NULL DEFAULT '', source TEXT NOT NULL DEFAULT '');
    CREATE INDEX IF NOT EXISTS test_run_pump ON test_run(profile_id,pump_name,started_ms);
    CREATE TABLE IF NOT EXISTS test_dose(id INTEGER PRIMARY KEY, run_id INTEGER NOT NULL REFERENCES test_run(id) ON DELETE CASCADE, nr INTEGER NOT NULL,
        requested_g REAL NOT NULL, dosed_g REAL, correction_pct REAL NOT NULL DEFAULT 0, target_g REAL, steps INTEGER, settings TEXT NOT NULL DEFAULT '{}',
        duration_s REAL, status TEXT NOT NULL, error TEXT NOT NULL DEFAULT '', excluded INTEGER NOT NULL DEFAULT 0, exclude_reason TEXT NOT NULL DEFAULT '', time_ms INTEGER NOT NULL);
    CREATE INDEX IF NOT EXISTS test_dose_run ON test_dose(run_id,nr);";
/// Lock wait of the UI connection; longer waits are reported, never sat out by the UI.
pub const UI_BUSY_TIMEOUT:std::time::Duration=std::time::Duration::from_millis(200);
/// Busy wait while opening (pragmas, schema check): other connections may be writing at that moment.
pub const OPEN_BUSY_TIMEOUT:std::time::Duration=std::time::Duration::from_secs(5);

#[derive(Clone,Copy,Debug,PartialEq,Eq,Hash)]
pub enum ProfileKind{Vsd,Keramisch}
impl ProfileKind{
    pub const ALL:[ProfileKind;2]=[ProfileKind::Vsd,ProfileKind::Keramisch];
    pub fn name(self)->&'static str{match self{ProfileKind::Vsd=>"VSD",ProfileKind::Keramisch=>"Keramisch"}}
    fn key(self)->&'static str{match self{ProfileKind::Vsd=>"vsd",ProfileKind::Keramisch=>"keramisch"}}
    fn from_key(k:&str)->Option<Self>{Self::ALL.into_iter().find(|p|p.key()==k)}
}

#[derive(Clone,Debug,PartialEq)]
pub struct Profile{pub id:i64,pub name:String,pub kind:ProfileKind}

/// One pump of a profile. `head` is None for the ceramic bench pump; `position` is the last known
/// position (VSD: revolutions; ceramic: steps of the app counter).
#[derive(Clone,Debug,PartialEq)]
pub struct PumpRow{pub id:i64,pub profile_id:i64,pub sort:i64,pub name:String,pub head:Option<Head>,pub node:u32,pub motor:u8,
    pub ingredient:String,pub enabled:bool,pub position:Option<f64>,pub pump_type:Option<String>,pub steps_rev:Option<i64>}
impl PumpRow{
    /// Filter of the Pompen page: text (name, ingredient, node), head (None = all) and active (None = all).
    pub fn matches(&self,text:&str,head:Option<Head>,active:Option<bool>)->bool{
        let t=text.trim().to_lowercase();
        (t.is_empty() || self.name.to_lowercase().contains(&t) || self.ingredient.to_lowercase().contains(&t) || self.node.to_string()==t)
            && head.is_none_or(|h|self.head==Some(h)) && active.is_none_or(|a|self.enabled==a)
    }
    pub fn to_vsd(&self)->VsdPump{VsdPump{name:self.name.clone(),node:self.node as u16,drive:self.motor,head:self.head.unwrap_or(Head::LH1),ingredient:self.ingredient.clone(),enabled:self.enabled,spr:self.steps_rev}}
}

fn head_str(h:Option<Head>)->Option<&'static str>{h.map(|h|h.name())}
fn head_from(s:Option<String>)->Option<Head>{match s.as_deref(){Some("LH1")=>Some(Head::LH1),Some("LH2")=>Some(Head::LH2),_=>None}}
fn e(x:rusqlite::Error)->String{format!("Database: {x}")}

pub struct Db{pub(crate) conn:Connection,path:PathBuf,
    /// True when this open created the database (schema + seed).
    pub created:bool}

impl Db{
    /// Open or create the database; runs migrations and, on a new database, the seed.
    /// Schema and seed are one transaction (R3): a start that broke off half-way leaves nothing behind, and a
    /// database with schema but without profiles is seeded on the next start.
    pub fn open(path:&Path)->Result<Self>{
        if let Some(d)=path.parent(){std::fs::create_dir_all(d).map_err(|x|format!("Datamap {}: {x}",d.display()))?;}
        // Several connections open at once (UI, log writer, head threads, production): the busy wait is set before any
        // pragma or migration check, generous while opening; a still-busy open is retried a few times (bounded).
        let mut last=String::new();
        for k in 0..4{
            if k>0{std::thread::sleep(std::time::Duration::from_millis(100*k));}
            match Self::open_once(path){Ok(d)=>return Ok(d),Err(x)=>{if !(x.contains("locked") || x.contains("busy")){return Err(x);}last=x;}}
        }
        Err(last)
    }
    fn open_once(path:&Path)->Result<Self>{
        let conn=Connection::open(path).map_err(|x|format!("Database {} openen: {x}",path.display()))?;
        conn.busy_timeout(OPEN_BUSY_TIMEOUT).map_err(e)?;
        // WAL: the log writer and the UI write concurrently; NORMAL sync is crash-safe in WAL mode.
        conn.pragma_update(None,"journal_mode","WAL").map_err(e)?;
        conn.pragma_update(None,"synchronous","NORMAL").map_err(e)?;
        conn.pragma_update(None,"foreign_keys","ON").map_err(e)?;
        let mut db=Self{conn,path:path.to_path_buf(),created:false};
        db.created=db.migrate()?;
        // Short wait for a lock from now on: the UI must never hang on the database; a locked write is reported.
        db.conn.busy_timeout(UI_BUSY_TIMEOUT).map_err(e)?;
        Ok(db)
    }
    pub fn set_busy_timeout(&self,t:std::time::Duration)->Result<()>{self.conn.busy_timeout(t).map_err(e)}
    pub fn path(&self)->&Path{&self.path}
    pub fn schema_version(&self)->Result<i64>{
        let exists:bool=self.conn.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='schema_version'",[],|r|r.get::<_,i64>(0)).map_err(e)?>0;
        if !exists{return Ok(0);}
        Ok(self.conn.query_row("SELECT max(version) FROM schema_version",[],|r|r.get::<_,Option<i64>>(0)).map_err(e)?.unwrap_or(0))
    }
    /// Stepwise migrations plus first-start seed in one transaction; returns true when seeded.
    /// A database of a newer program is refused, never changed.
    fn migrate(&mut self)->Result<bool>{
        let v=self.schema_version()?;
        if v>SCHEMA_VERSION{return Err(format!("Database {} heeft schemaversie {v}; dit programma kent tot versie {SCHEMA_VERSION}. Gebruik een nieuwere Pump Studio.",self.path.display()));}
        // Up to date and seeded: read only, no write lock (most opens: head threads, production, tests).
        if v==SCHEMA_VERSION{
            let n:i64=self.conn.query_row("SELECT count(*) FROM profile",[],|r|r.get(0)).map_err(e)?;
            if n>0{return Ok(false);}
        }
        let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
        if v<1{
            tx.execute_batch("
                CREATE TABLE IF NOT EXISTS schema_version(version INTEGER NOT NULL);
                CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                CREATE TABLE IF NOT EXISTS profile(id INTEGER PRIMARY KEY, name TEXT NOT NULL UNIQUE, kind TEXT NOT NULL UNIQUE);
                CREATE TABLE IF NOT EXISTS settings(profile_id INTEGER NOT NULL REFERENCES profile(id), key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(profile_id,key));
                CREATE TABLE IF NOT EXISTS ingredient(id INTEGER PRIMARY KEY, profile_id INTEGER NOT NULL REFERENCES profile(id), name TEXT NOT NULL,
                    density REAL, notes TEXT, UNIQUE(profile_id,name));
                CREATE TABLE IF NOT EXISTS pump(id INTEGER PRIMARY KEY, profile_id INTEGER NOT NULL REFERENCES profile(id), sort INTEGER NOT NULL,
                    name TEXT NOT NULL, head TEXT, node INTEGER NOT NULL, motor INTEGER NOT NULL,
                    ingredient_id INTEGER REFERENCES ingredient(id), enabled INTEGER NOT NULL DEFAULT 1, position REAL,
                    pump_type TEXT, steps_rev INTEGER, notes TEXT,
                    UNIQUE(profile_id,name), UNIQUE(profile_id,node,motor));
                CREATE TABLE IF NOT EXISTS log(id INTEGER PRIMARY KEY, time TEXT NOT NULL, level TEXT NOT NULL, area TEXT NOT NULL, text TEXT NOT NULL);
                CREATE INDEX IF NOT EXISTS log_time ON log(time);
                CREATE INDEX IF NOT EXISTS log_level_area ON log(level,area);
                ").map_err(e)?;
        }
        if v==1{
            // Iteration 1 stored VSD positions in revolutions; from v2 every position is in steps. Steps per
            // revolution: the pump's own value, else the configured machine value (not a fixed 51 200).
            let raw:Option<String>=tx.query_row("SELECT value FROM settings WHERE key='machine' AND profile_id=(SELECT id FROM profile WHERE kind='vsd')",[],|r|r.get(0)).optional().map_err(e)?;
            let spr=raw.and_then(|j|serde_json::from_str::<MachineConfig>(&j).ok()).map(|m|m.spr).filter(|v|*v>0).unwrap_or(MachineConfig::default().spr);
            tx.execute("UPDATE pump SET position=round(position*coalesce(steps_rev,?1)) WHERE position IS NOT NULL AND profile_id=(SELECT id FROM profile WHERE kind='vsd')",[spr]).map_err(e)?;
        }
        if v<2{tx.execute_batch(V2).map_err(e)?;}
        if v<3{tx.execute_batch(V3).map_err(e)?;}
        if v<4{tx.execute_batch(V4).map_err(e)?;}
        if v<5{tx.execute_batch(V5).map_err(e)?;}
        if v<6{tx.execute_batch(V6).map_err(e)?;}
        if v<SCHEMA_VERSION{tx.execute_batch(&format!("DELETE FROM schema_version;INSERT INTO schema_version(version) VALUES({SCHEMA_VERSION});")).map_err(e)?;}
        let profiles:i64=tx.query_row("SELECT count(*) FROM profile",[],|r|r.get(0)).map_err(e)?;
        let seeded=profiles==0;
        if seeded{Self::seed(&tx)?;}
        tx.commit().map_err(e)?;
        Ok(seeded)
    }
    /// First start: VSD profile with the 192 Camelot pumps, Keramisch profile with one pump (model defaults).
    fn seed(tx:&rusqlite::Transaction)->Result<()>{
        // A database that lost its profiles (never committed together with them) may hold orphan rows.
        tx.execute_batch("DELETE FROM pump;DELETE FROM ingredient;DELETE FROM settings;").map_err(e)?;
        tx.execute("INSERT INTO profile(name,kind) VALUES('VSD','vsd'),('Keramisch','keramisch')",[]).map_err(e)?;
        let vsd:i64=tx.query_row("SELECT id FROM profile WHERE kind='vsd'",[],|r|r.get(0)).map_err(e)?;
        let ker:i64=tx.query_row("SELECT id FROM profile WHERE kind='keramisch'",[],|r|r.get(0)).map_err(e)?;
        let mut machine=MachineConfig::default();let pumps=std::mem::take(&mut machine.pumps);
        insert_vsd_pumps(tx,vsd,&pumps)?;
        let bench=Config::default();
        tx.execute("INSERT INTO pump(profile_id,sort,name,head,node,motor,enabled,pump_type,steps_rev) VALUES(?1,0,'Keramisch 1',NULL,?2,?3,1,?4,?5)",
            params![ker,bench.node,bench.motor,bench.pump_type,bench.steps_rev]).map_err(e)?;
        for (pid,key,value) in [(vsd,"machine",json(&machine)?),(vsd,"python",json(&default_python())?),(ker,"bench",json(&bench)?),(ker,"python",json(&default_python())?),(ker,"repo",json(&default_repo())?)]{
            tx.execute("INSERT INTO settings(profile_id,key,value) VALUES(?1,?2,?3)",params![pid,key,value]).map_err(e)?;
        }
        tx.execute("INSERT OR REPLACE INTO meta(key,value) VALUES('active_profile','vsd')",[]).map_err(e)?;
        Ok(())
    }
    #[cfg(test)] pub(crate) fn conn(&self)->&Connection{&self.conn}

    // ---------- profiles and app-wide values ----------
    pub fn profiles(&self)->Result<Vec<Profile>>{
        let mut st=self.conn.prepare("SELECT id,name,kind FROM profile ORDER BY id").map_err(e)?;
        let rows=st.query_map([],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?))).map_err(e)?;
        let mut out=vec![];
        for row in rows{let (id,name,kind)=row.map_err(e)?;if let Some(kind)=ProfileKind::from_key(&kind){out.push(Profile{id,name,kind});}}
        Ok(out)
    }
    pub fn profile(&self,kind:ProfileKind)->Result<Profile>{
        self.profiles()?.into_iter().find(|p|p.kind==kind).ok_or_else(||format!("Profiel {} ontbreekt in database",kind.name()))
    }
    pub fn meta(&self,key:&str)->Result<Option<String>>{
        self.conn.query_row("SELECT value FROM meta WHERE key=?1",[key],|r|r.get(0)).optional().map_err(e)
    }
    pub fn set_meta(&self,key:&str,value:&str)->Result<()>{
        self.conn.execute("INSERT INTO meta(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value",[key,value]).map(|_|()).map_err(e)
    }
    pub fn active_profile(&self)->Result<ProfileKind>{
        Ok(self.meta("active_profile")?.and_then(|k|ProfileKind::from_key(&k)).unwrap_or(ProfileKind::Vsd))
    }
    pub fn set_active_profile(&self,kind:ProfileKind)->Result<()>{self.set_meta("active_profile",kind.key())}

    // ---------- settings (JSON per key and profile) ----------
    pub fn setting<T:DeserializeOwned>(&self,profile:ProfileKind,key:&str)->Result<Option<T>>{
        let id=self.profile(profile)?.id;
        let raw:Option<String>=self.conn.query_row("SELECT value FROM settings WHERE profile_id=?1 AND key=?2",params![id,key],|r|r.get(0)).optional().map_err(e)?;
        match raw{None=>Ok(None),Some(s)=>serde_json::from_str(&s).map(Some).map_err(|x|format!("Instelling {key} onleesbaar: {x}"))}
    }
    pub fn set_setting<T:Serialize>(&self,profile:ProfileKind,key:&str,value:&T)->Result<()>{
        let id=self.profile(profile)?.id;
        self.conn.execute("INSERT INTO settings(profile_id,key,value) VALUES(?1,?2,?3) ON CONFLICT(profile_id,key) DO UPDATE SET value=excluded.value",
            params![id,key,json(value)?]).map(|_|()).map_err(e)
    }
    /// VSD connection and motion settings (pump list empty; pumps live in the pump table).
    pub fn machine_settings(&self)->Result<MachineConfig>{
        let mut m:MachineConfig=self.setting(ProfileKind::Vsd,"machine")?.unwrap_or_default();m.pumps.clear();Ok(m)
    }
    /// Full machine config for the CAN group: settings plus the pump table of the VSD profile.
    pub fn machine_config(&self)->Result<MachineConfig>{
        let mut m=self.machine_settings()?;let id=self.profile(ProfileKind::Vsd)?.id;
        m.pumps=self.pumps(id)?.iter().map(PumpRow::to_vsd).collect();Ok(m)
    }
    pub fn bench_config(&self)->Result<Config>{Ok(self.setting(ProfileKind::Keramisch,"bench")?.unwrap_or_default())}
    /// Store the bench settings and keep the single ceramic pump row (CAN address, type) in step.
    pub fn set_bench_config(&self,cfg:&Config)->Result<()>{
        self.set_setting(ProfileKind::Keramisch,"bench",cfg)?;
        let id=self.profile(ProfileKind::Keramisch)?.id;
        self.conn.execute("UPDATE pump SET node=?2,motor=?3,pump_type=?4,steps_rev=?5 WHERE id=(SELECT id FROM pump WHERE profile_id=?1 ORDER BY sort LIMIT 1)",
            params![id,cfg.node,cfg.motor,cfg.pump_type,cfg.steps_rev]).map(|_|()).map_err(e)
    }

    // ---------- pumps and ingredients ----------
    pub fn pumps(&self,profile_id:i64)->Result<Vec<PumpRow>>{
        let mut st=self.conn.prepare("SELECT p.id,p.profile_id,p.sort,p.name,p.head,p.node,p.motor,coalesce(i.name,''),p.enabled,p.position,p.pump_type,p.steps_rev
            FROM pump p LEFT JOIN ingredient i ON i.id=p.ingredient_id WHERE p.profile_id=?1 ORDER BY p.sort,p.id").map_err(e)?;
        let rows=st.query_map([profile_id],|r|Ok(PumpRow{id:r.get(0)?,profile_id:r.get(1)?,sort:r.get(2)?,name:r.get(3)?,head:head_from(r.get(4)?),
            node:r.get(5)?,motor:r.get(6)?,ingredient:r.get(7)?,enabled:r.get::<_,i64>(8)?!=0,position:r.get(9)?,pump_type:r.get(10)?,steps_rev:r.get(11)?})).map_err(e)?;
        rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    /// Ingredient row by name for this profile, created when new; empty name = no ingredient.
    pub fn ensure_ingredient(&self,profile_id:i64,name:&str)->Result<Option<i64>>{
        let name=name.trim();if name.is_empty(){return Ok(None);}
        self.conn.execute("INSERT OR IGNORE INTO ingredient(profile_id,name) VALUES(?1,?2)",params![profile_id,name]).map_err(e)?;
        self.conn.query_row("SELECT id FROM ingredient WHERE profile_id=?1 AND name=?2",params![profile_id,name],|r|r.get(0)).map(Some).map_err(e)
    }
    pub fn ingredients(&self,profile_id:i64)->Result<Vec<String>>{
        let mut st=self.conn.prepare("SELECT name FROM ingredient WHERE profile_id=?1 ORDER BY name").map_err(e)?;
        let rows=st.query_map([profile_id],|r|r.get(0)).map_err(e)?;
        rows.collect::<std::result::Result<Vec<_>,_>>().map_err(e)
    }
    /// Editable fields of the Pompen page: head, ingredient (free text becomes an ingredient row) and active.
    pub fn update_pump(&self,row:&PumpRow)->Result<()>{
        let ing=self.ensure_ingredient(row.profile_id,&row.ingredient)?;
        let n=self.conn.execute("UPDATE pump SET head=?2,ingredient_id=?3,enabled=?4 WHERE id=?1",params![row.id,head_str(row.head),ing,row.enabled as i64]).map_err(e)?;
        if n!=1{return Err(format!("Pomp {} niet gevonden in database",row.name));}Ok(())
    }
    /// Positions after one pump command, all in one transaction (P6).
    pub fn set_pump_positions(&mut self,rows:&[(i64,Option<f64>)])->Result<()>{
        if rows.is_empty(){return Ok(());}
        let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
        {let mut st=tx.prepare_cached("UPDATE pump SET position=?2 WHERE id=?1").map_err(e)?;for (id,p) in rows{st.execute(params![id,p]).map_err(e)?;}}
        tx.commit().map_err(e)
    }
    pub fn set_pump_position(&self,id:i64,position:Option<f64>)->Result<()>{
        self.conn.execute("UPDATE pump SET position=?2 WHERE id=?1",params![id,position]).map(|_|()).map_err(e)
    }

    // ---------- log index ----------
    pub fn insert_logs(&mut self,lines:&[Line])->Result<()>{
        let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
        {let mut st=tx.prepare_cached("INSERT INTO log(time,level,area,text) VALUES(?1,?2,?3,?4)").map_err(e)?;
            for l in lines{st.execute(params![l.time,l.level.name(),l.area,l.text]).map_err(e)?;}}
        tx.commit().map_err(e)
    }
    pub fn log_count(&self,like:&str)->Result<i64>{
        self.conn.query_row("SELECT count(*) FROM log WHERE text LIKE ?1",[format!("%{like}%")],|r|r.get(0)).map_err(e)
    }

    // ---------- one-time import of the previous app's files ----------
    /// `machine/machine.json` of the tab-based app: VSD settings and pump list. Runs once (meta flag);
    /// the old file is left in place. The old app never stored the ceramic bench settings in a file.
    pub fn import_legacy(&mut self,root:&Path)->Result<Option<String>>{
        if self.meta("import_machine_json")?.is_some(){return Ok(None);}
        let path=root.join("machine").join("machine.json");
        let note=match std::fs::read(&path){
            Err(_)=>None,
            Ok(bytes)=>{
                let mut cfg:MachineConfig=serde_json::from_slice(&bytes).map_err(|x|format!("{} onleesbaar: {x}",path.display()))?;
                cfg.validate().map_err(|x|format!("{} ongeldig: {x}",path.display()))?;
                let pumps=std::mem::take(&mut cfg.pumps);let vsd=self.profile(ProfileKind::Vsd)?.id;
                let tx=self.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).map_err(e)?;
                tx.execute("DELETE FROM pump WHERE profile_id=?1",[vsd]).map_err(e)?;
                insert_vsd_pumps(&tx,vsd,&pumps)?;
                tx.execute("INSERT INTO settings(profile_id,key,value) VALUES(?1,'machine',?2) ON CONFLICT(profile_id,key) DO UPDATE SET value=excluded.value",params![vsd,json(&cfg)?]).map_err(e)?;
                tx.commit().map_err(e)?;
                Some(format!("{} pompen en VSD-instellingen overgenomen uit {}",pumps.len(),path.display()))
            }
        };
        self.set_meta("import_machine_json",if note.is_some(){"done"}else{"none"})?;
        Ok(note)
    }
}

fn insert_vsd_pumps(tx:&rusqlite::Transaction,profile_id:i64,pumps:&[VsdPump])->Result<()>{
    for (k,p) in pumps.iter().enumerate(){
        let ing:Option<i64>=if p.ingredient.trim().is_empty(){None}else{
            tx.execute("INSERT OR IGNORE INTO ingredient(profile_id,name) VALUES(?1,?2)",params![profile_id,p.ingredient.trim()]).map_err(e)?;
            Some(tx.query_row("SELECT id FROM ingredient WHERE profile_id=?1 AND name=?2",params![profile_id,p.ingredient.trim()],|r|r.get(0)).map_err(e)?)};
        tx.execute("INSERT INTO pump(profile_id,sort,name,head,node,motor,ingredient_id,enabled) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![profile_id,k as i64,p.name,p.head.name(),p.node,p.drive,ing,p.enabled as i64]).map_err(|x|format!("Pomp {}: {}",p.name,e(x)))?;
    }
    Ok(())
}
fn json<T:Serialize+?Sized>(v:&T)->Result<String>{serde_json::to_string(v).map_err(|x|x.to_string())}
pub fn default_python()->String{r"C:\Users\Thomas\CAN\.venv\Scripts\python.exe".into()}
pub fn default_repo()->String{r"C:\Users\Thomas\CAN".into()}
/// Check that the seeded VSD pumps equal `machine::default_pumps()` (used by the tests and the smoke test).
pub fn same_as_default(rows:&[PumpRow])->bool{
    let d=default_pumps();
    rows.len()==d.len() && rows.iter().zip(&d).all(|(r,p)|r.name==p.name && r.node==p.node as u32 && r.motor==p.drive && r.head==Some(p.head) && r.ingredient==p.ingredient && r.enabled)
}

/// Background writer of the log index: own connection, batches lines in one transaction.
/// Errors go to the day file only (never back into the sink, so no loop).
pub fn spawn_log_writer(path:PathBuf)->Result<std::sync::mpsc::Sender<Line>>{
    let mut db=Db::open(&path)?;
    // Background thread: it may wait for the UI's short writes.
    db.set_busy_timeout(std::time::Duration::from_millis(2000))?;
    let (tx,rx)=std::sync::mpsc::channel::<Line>();
    std::thread::Builder::new().name("db-log".into()).spawn(move||{
        let mut failed=false;
        while let Ok(first)=rx.recv(){
            let mut batch=vec![first];
            while let Ok(l)=rx.try_recv(){batch.push(l);if batch.len()>=500{break;}}
            if let Err(x)=db.insert_logs(&batch){
                if !failed{crate::applog::write_local(crate::applog::Level::Error,"DATABASE",&format!("Logindex schrijven mislukt: {x}"));}failed=true;
            }else{failed=false;}
        }
    }).map_err(|x|x.to_string())?;
    Ok(tx)
}

#[cfg(test)] mod tests{
    use super::*;
    pub fn temp(name:&str)->PathBuf{
        let d=std::env::temp_dir().join(format!("pumpstudio-db-{name}-{}-{}",std::process::id(),std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
        std::fs::create_dir_all(&d).unwrap();d
    }
    /// Windows keeps the file locked until every connection is closed (also the log writer's): retry briefly.
    pub fn cleanup(d:&Path){for _ in 0..100{if std::fs::remove_dir_all(d).is_ok() || !d.exists(){return;}std::thread::sleep(std::time::Duration::from_millis(20));}}
    #[test]fn fresh_db_has_schema_profiles_and_pumps(){
        let dir=temp("fresh");let db=Db::open(&dir.join("data/pumpstudio.db")).unwrap();
        assert!(db.created);assert_eq!(db.schema_version().unwrap(),SCHEMA_VERSION);
        let p=db.profiles().unwrap();assert_eq!(p.iter().map(|p|p.kind).collect::<Vec<_>>(),vec![ProfileKind::Vsd,ProfileKind::Keramisch]);
        let vsd=db.pumps(p[0].id).unwrap();assert_eq!(vsd.len(),192);assert!(same_as_default(&vsd));
        let get=|n:&str|vsd.iter().find(|r|r.name==n).unwrap().clone();
        assert_eq!((get("P_A01").node,get("P_A01").motor,get("P_A01").head),(610,1,Some(Head::LH2)));
        assert_eq!((get("P_B48").node,get("P_B48").motor,get("P_B48").head),(621,8,Some(Head::LH2)));
        assert_eq!((get("P_C01").node,get("P_C01").head),(622,Some(Head::LH1)));
        assert_eq!((get("P_D48").node,get("P_D48").motor,get("P_D48").head),(633,8,Some(Head::LH1)));
        let ker=db.pumps(p[1].id).unwrap();assert_eq!(ker.len(),1);let c=Config::default();
        assert_eq!((ker[0].node,ker[0].motor,ker[0].head,ker[0].pump_type.as_deref()),(c.node,c.motor,None,Some("INV-13")));
        assert_eq!(db.bench_config().unwrap(),c);assert_eq!(db.machine_config().unwrap().pumps.len(),192);
        db.machine_config().unwrap().validate().unwrap();
        assert_eq!(db.active_profile().unwrap(),ProfileKind::Vsd);
        drop(db);cleanup(&dir);
    }
    #[test]fn reopen_keeps_edits_and_does_not_reseed(){
        let dir=temp("reopen");let path=dir.join("pumpstudio.db");
        {let db=Db::open(&path).unwrap();let vsd=db.profile(ProfileKind::Vsd).unwrap().id;
            let mut r=db.pumps(vsd).unwrap()[5].clone();r.head=Some(Head::LH1);r.ingredient="Water".into();r.enabled=false;db.update_pump(&r).unwrap();
            db.set_pump_position(r.id,Some(1.25)).unwrap();db.set_active_profile(ProfileKind::Keramisch).unwrap();
            let mut b=db.bench_config().unwrap();b.motor=5;b.node=611;db.set_bench_config(&b).unwrap();
            let mut m=db.machine_settings().unwrap();m.plc.ip="10.6.1.99".into();db.set_setting(ProfileKind::Vsd,"machine",&m).unwrap();}
        let db=Db::open(&path).unwrap();assert!(!db.created);
        let vsd=db.profile(ProfileKind::Vsd).unwrap().id;let rows=db.pumps(vsd).unwrap();assert_eq!(rows.len(),192);
        let r=&rows[5];assert_eq!((r.head,r.ingredient.as_str(),r.enabled,r.position),(Some(Head::LH1),"Water",false,Some(1.25)));
        assert!(db.ingredients(vsd).unwrap().contains(&"Water".to_string()));
        assert_eq!(db.active_profile().unwrap(),ProfileKind::Keramisch);
        let k=db.pumps(db.profile(ProfileKind::Keramisch).unwrap().id).unwrap();assert_eq!((k[0].node,k[0].motor),(611,5));
        assert_eq!(db.bench_config().unwrap().motor,5);assert_eq!(db.machine_settings().unwrap().plc.ip,"10.6.1.99");
        drop(db);cleanup(&dir);
    }
    #[test]fn newer_schema_is_refused(){
        let dir=temp("newer");let path=dir.join("pumpstudio.db");
        {let db=Db::open(&path).unwrap();db.conn.execute("INSERT INTO schema_version(version) VALUES(99)",[]).unwrap();}
        let err=Db::open(&path).err().unwrap();assert!(err.contains("schemaversie 99"),"{err}");
        cleanup(&dir);
    }
    #[test]fn half_created_db_recovers_on_next_start(){
        // (a) schema committed but seed missing (as a pre-fix crash could leave): seeded on open.
        let dir=temp("half");let path=dir.join("p.db");
        {let db=Db::open(&path).unwrap();db.conn().execute_batch("PRAGMA foreign_keys=OFF;DELETE FROM pump;DELETE FROM settings;DELETE FROM ingredient;DELETE FROM profile;").unwrap();}
        let db=Db::open(&path).unwrap();assert!(db.created);assert_eq!(db.profiles().unwrap().len(),2);
        assert_eq!(db.pumps(db.profile(ProfileKind::Vsd).unwrap().id).unwrap().len(),192);assert_eq!(db.schema_version().unwrap(),SCHEMA_VERSION);drop(db);
        // (b) creation broken off inside the transaction: nothing stays, the next open builds everything.
        let path2=dir.join("q.db");
        {let mut c=Connection::open(&path2).unwrap();let tx=c.transaction().unwrap();tx.execute_batch("CREATE TABLE schema_version(version INTEGER NOT NULL);CREATE TABLE profile(id INTEGER PRIMARY KEY);").unwrap();drop(tx);}
        let db=Db::open(&path2).unwrap();assert!(db.created);assert_eq!(db.pumps(db.profile(ProfileKind::Keramisch).unwrap().id).unwrap().len(),1);
        // (c) schema_version table without rows (old partial file): treated as new.
        let path3=dir.join("r.db");
        {let c=Connection::open(&path3).unwrap();c.execute_batch("CREATE TABLE schema_version(version INTEGER NOT NULL);").unwrap();}
        let db3=Db::open(&path3).unwrap();assert!(db3.created && db3.schema_version().unwrap()==SCHEMA_VERSION);
        drop(db);drop(db3);cleanup(&dir);
    }
    #[test]fn positions_in_one_transaction(){
        let dir=temp("pos");let mut db=Db::open(&dir.join("p.db")).unwrap();let vsd=db.profile(ProfileKind::Vsd).unwrap().id;
        let rows=db.pumps(vsd).unwrap();let upd:Vec<(i64,Option<f64>)>=rows.iter().take(5).enumerate().map(|(k,r)|(r.id,Some(k as f64*0.25))).collect();
        db.set_pump_positions(&upd).unwrap();let after=db.pumps(vsd).unwrap();
        for (k,r) in after.iter().take(5).enumerate(){assert_eq!(r.position,Some(k as f64*0.25));}
        // Locked by another writer: the UI connection gives up after its short timeout and reports it.
        let other=Connection::open(dir.join("p.db")).unwrap();other.execute_batch("BEGIN IMMEDIATE;UPDATE meta SET value=value;").unwrap();
        let t=std::time::Instant::now();let r=db.set_pump_positions(&upd);
        assert!(r.is_err() && t.elapsed()<std::time::Duration::from_millis(1500),"{r:?} {:?}",t.elapsed());
        other.execute_batch("ROLLBACK;").unwrap();drop(other);drop(db);cleanup(&dir);
    }
    #[test]fn legacy_machine_json_imported_once(){
        let dir=temp("legacy");let mut m=MachineConfig::default();m.pumps[0].ingredient="Rood".into();m.pumps[0].head=Head::LH1;m.pumps.truncate(10);m.can_channel=1;
        std::fs::create_dir_all(dir.join("machine")).unwrap();std::fs::write(dir.join("machine/machine.json"),serde_json::to_vec(&m).unwrap()).unwrap();
        let mut db=Db::open(&dir.join("data/pumpstudio.db")).unwrap();
        let note=db.import_legacy(&dir).unwrap().unwrap();assert!(note.contains("10 pompen"),"{note}");
        let rows=db.pumps(db.profile(ProfileKind::Vsd).unwrap().id).unwrap();assert_eq!(rows.len(),10);
        assert_eq!((rows[0].ingredient.as_str(),rows[0].head),("Rood",Some(Head::LH1)));assert_eq!(db.machine_settings().unwrap().can_channel,1);
        // Second start: no new import, even when the file changed; the file itself stays.
        m.pumps.truncate(3);std::fs::write(dir.join("machine/machine.json"),serde_json::to_vec(&m).unwrap()).unwrap();
        assert!(db.import_legacy(&dir).unwrap().is_none());assert_eq!(db.pumps(db.profile(ProfileKind::Vsd).unwrap().id).unwrap().len(),10);
        assert!(dir.join("machine/machine.json").exists());
        // Without old file: flag set, nothing changed.
        let dir2=temp("legacy2");let mut db2=Db::open(&dir2.join("x.db")).unwrap();assert!(db2.import_legacy(&dir2).unwrap().is_none());assert_eq!(db2.meta("import_machine_json").unwrap().as_deref(),Some("none"));
        drop(db);drop(db2);cleanup(&dir);cleanup(&dir2);
    }
    #[test]fn pump_filter(){
        let dir=temp("filter");let db=Db::open(&dir.join("p.db")).unwrap();let rows=db.pumps(db.profile(ProfileKind::Vsd).unwrap().id).unwrap();
        let n=|t:&str,h:Option<Head>,a:Option<bool>|rows.iter().filter(|r|r.matches(t,h,a)).count();
        assert_eq!(n("",None,None),192);assert_eq!(n("",Some(Head::LH1),None),96);assert_eq!(n("p_a",None,None),48);assert_eq!(n("I_D0",Some(Head::LH1),None),9);
        assert_eq!(n("610",None,None),8);assert_eq!(n("p_a",Some(Head::LH1),None),0);assert_eq!(n("",None,Some(false)),0);
        let mut r=rows[0].clone();r.enabled=false;assert!(r.matches("",None,Some(false)) && !r.matches("",None,Some(true)));
        drop(db);cleanup(&dir);
    }
    #[test]fn log_writer_indexes_lines(){
        let dir=temp("log");let path=dir.join("p.db");let _db=Db::open(&path).unwrap();
        let tx=spawn_log_writer(path.clone()).unwrap();
        for i in 0..20{tx.send(Line{time:format!("2026-11-02 10:00:{i:02}.000"),level:crate::applog::Level::Info,area:"TEST".into(),text:format!("regel-db {i}")}).unwrap();}
        drop(tx);let db=Db::open(&path).unwrap();
        let t0=std::time::Instant::now();while db.log_count("regel-db").unwrap()<20 && t0.elapsed().as_secs()<5{std::thread::sleep(std::time::Duration::from_millis(20));}
        assert_eq!(db.log_count("regel-db").unwrap(),20);
        drop(db);drop(_db);cleanup(&dir);
    }
    #[test]fn open_while_others_write_does_not_fail(){
        // Writer holds a write lock for 600 ms (longer than the UI busy wait) while 8 threads open the database.
        let dir=temp("contend");let path=dir.join("data/pumpstudio.db");drop(Db::open(&path).unwrap());
        let p=path.clone();let w=std::thread::spawn(move||{let mut d=Db::open(&p).unwrap();let tx=d.conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate).unwrap();
            tx.execute("INSERT INTO meta(key,value) VALUES('x','1')",[]).unwrap();std::thread::sleep(std::time::Duration::from_millis(600));tx.commit().unwrap();});
        std::thread::sleep(std::time::Duration::from_millis(50));
        let hs:Vec<_>=(0..8).map(|_|{let p=path.clone();std::thread::spawn(move||Db::open(&p).map(|d|d.schema_version().unwrap()))}).collect();
        for h in hs{assert_eq!(h.join().unwrap(),Ok(SCHEMA_VERSION));}
        w.join().unwrap();let _=std::fs::remove_dir_all(dir);
    }
}
