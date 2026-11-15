//! Thin wrapper around rodio: one track at a time, no effects.

use std::{fs::File, path::Path, time::Duration};

use rodio::{Decoder, DeviceSinkBuilder, MixerDeviceSink, Player};

pub struct Audio {
    // Dropping the device sink kills playback, so keep it alive.
    _device: MixerDeviceSink,
    player: Player,
}

impl Audio {
    pub fn open() -> Result<Self, String> {
        let device = DeviceSinkBuilder::open_default_sink().map_err(|e| e.to_string())?;
        let player = Player::connect_new(device.mixer());
        Ok(Self {
            _device: device,
            player,
        })
    }

    /// Starts playing `path` from the beginning.
    pub fn play(&self, path: &Path, volume: f32) -> Result<(), String> {
        let file = File::open(path).map_err(|e| e.to_string())?;
        let len = file.metadata().map_err(|e| e.to_string())?.len();
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();

        let mut builder = Decoder::builder()
            .with_data(file)
            .with_byte_len(len)
            .with_seekable(true);
        if !ext.is_empty() {
            builder = builder.with_hint(&ext);
        }
        let source = builder.build().map_err(|e| e.to_string())?;

        self.player.stop();
        self.player.set_volume(volume);
        self.player.append(source);
        // `stop()` leaves the player paused.
        self.player.play();
        Ok(())
    }

    pub fn pause(&self) {
        self.player.pause();
    }

    pub fn resume(&self) {
        self.player.play();
    }

    pub fn is_paused(&self) -> bool {
        self.player.is_paused()
    }

    pub fn set_volume(&self, volume: f32) {
        self.player.set_volume(volume);
    }

    pub fn seek(&self, pos: Duration) {
        let _ = self.player.try_seek(pos);
    }

    pub fn position(&self) -> Duration {
        self.player.get_pos()
    }

    /// True when the current track ran out.
    pub fn finished(&self) -> bool {
        self.player.empty()
    }

    pub fn stop(&self) {
        self.player.stop();
    }
}
