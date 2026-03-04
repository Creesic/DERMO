use crate::core::CanMessage;
use crate::playback::{PlaybackConfig, PlaybackState};
use chrono::{DateTime, Utc, Duration};
use std::time::{Duration as StdDuration, Instant};

/// Playback engine for CAN data
pub struct PlaybackEngine {
    messages: Vec<CanMessage>,
    config: PlaybackConfig,
    state: PlaybackState,
    current_position: usize,
    virtual_start_time: Option<Instant>,
    real_start_time: Option<DateTime<Utc>>,
}

impl PlaybackEngine {
    pub fn new(messages: Vec<CanMessage>) -> Self {
        Self {
            messages,
            config: PlaybackConfig {
                speed: 1.0,
                loop_playback: false,
            },
            state: PlaybackState::Stopped,
            current_position: 0,
            virtual_start_time: None,
            real_start_time: None,
        }
    }

    /// Get current playback position (index into messages)
    pub fn position(&self) -> usize {
        self.current_position
    }

    /// Get total number of messages
    pub fn total_messages(&self) -> usize {
        self.messages.len()
    }

    /// Append messages (for streaming load). Messages must be in timestamp order.
    pub fn append_messages(&mut self, msgs: &[CanMessage]) {
        self.messages.extend_from_slice(msgs);
    }

    /// Get current playback state
    pub fn state(&self) -> PlaybackState {
        self.state
    }

    /// Set playback speed
    pub fn set_speed(&mut self, speed: f64) {
        self.config.speed = speed.max(0.1).min(10.0);
        self.virtual_start_time = None; // Reset timing when speed changes
    }

    /// Get current playback speed
    pub fn speed(&self) -> f64 {
        self.config.speed
    }

    /// Start/resume playback
    pub fn play(&mut self) {
        // If at end, restart from beginning
        if self.current_position >= self.messages.len() {
            if self.messages.is_empty() {
                return; // No messages to play
            }
            self.current_position = 0;
        }

        self.state = PlaybackState::Playing;
        self.virtual_start_time = Some(Instant::now());
        self.real_start_time = Some(self.current_time().unwrap_or_else(|| Utc::now()));
    }

    /// Pause playback
    pub fn pause(&mut self) {
        self.state = PlaybackState::Paused;
        self.virtual_start_time = None;
    }

    /// Stop playback and reset
    pub fn stop(&mut self) {
        self.state = PlaybackState::Stopped;
        self.current_position = 0;
        self.virtual_start_time = None;
        self.real_start_time = None;
    }

    /// Seek to a specific time in the log
    pub fn seek_to_time(&mut self, time: Option<DateTime<Utc>>) {
        if let Some(target_time) = time {
            // Find the message closest to target_time
            self.current_position = self.messages
                .binary_search_by(|msg| {
                    msg.timestamp.cmp(&target_time)
                })
                .unwrap_or_else(|pos| pos);

            self.virtual_start_time = None;
        }
    }

    /// Seek to a specific position (index)
    pub fn seek_to_position(&mut self, pos: usize) {
        self.current_position = pos.clamp(0, self.messages.len());
        self.virtual_start_time = None;
    }

    /// Step forward by one frame
    pub fn step_forward(&mut self) {
        if self.current_position < self.messages.len().saturating_sub(1) {
            self.current_position += 1;
        }
        // Pause when stepping
        self.state = PlaybackState::Paused;
        self.virtual_start_time = None;
    }

    /// Step backward by one frame
    pub fn step_back(&mut self) {
        if self.current_position > 0 {
            self.current_position -= 1;
        }
        // Pause when stepping
        self.state = PlaybackState::Paused;
        self.virtual_start_time = None;
    }

    /// Check if currently playing
    pub fn is_playing(&self) -> bool {
        self.state == PlaybackState::Playing
    }

    /// Get current timestamp in the log
    pub fn current_time(&self) -> Option<DateTime<Utc>> {
        if self.messages.is_empty() {
            return None;
        }
        // If position is past the end, return the last message's timestamp
        if self.current_position >= self.messages.len() {
            self.messages.last().map(|m| m.timestamp)
        } else {
            self.messages.get(self.current_position).map(|m| m.timestamp)
        }
    }

    /// Get start time of the log
    pub fn start_time(&self) -> Option<DateTime<Utc>> {
        self.messages.first().map(|m| m.timestamp)
    }

    /// Get end time of the log
    pub fn end_time(&self) -> Option<DateTime<Utc>> {
        self.messages.last().map(|m| m.timestamp)
    }

    /// Update playback state (call each frame)
    pub fn update(&mut self, _delta_time: StdDuration) {
        if self.state != PlaybackState::Playing {
            return;
        }

        // Reinitialize virtual_start_time if it was reset (e.g., by seeking)
        if self.virtual_start_time.is_none() {
            if let Some(real_start) = self.real_start_time {
                // Calculate new virtual_start based on current position
                let current_time = self.messages.get(self.current_position)
                    .map(|m| m.timestamp)
                    .unwrap_or(real_start);
                let elapsed_so_far = (current_time - real_start).num_milliseconds() as f64 / 1000.0;
                if elapsed_so_far < 0.0 {
                    // Seeked backward past real_start - restart timing from current position
                    self.real_start_time = Some(current_time);
                }
                // Clamp to 0: Duration::from_secs_f64 panics on negative
                let elapsed_secs = (elapsed_so_far / self.config.speed).max(0.0);
                let elapsed_instant = StdDuration::from_secs_f64(elapsed_secs);
                self.virtual_start_time = Some(Instant::now() - elapsed_instant);
            }
        }

        if let Some(virtual_start) = self.virtual_start_time {
            let elapsed = virtual_start.elapsed();
            let scaled_elapsed = StdDuration::from_secs_f64(elapsed.as_secs_f64() * self.config.speed);

            if let Some(real_start) = self.real_start_time {
                let target_time = real_start + scaled_elapsed;

                // Find new position based on target time
                let new_pos = self.messages
                    .binary_search_by(|msg| msg.timestamp.cmp(&target_time))
                    .unwrap_or_else(|pos| pos);

                self.current_position = new_pos;

                // Check if we've reached the end
                if self.current_position >= self.messages.len() {
                    if self.config.loop_playback {
                        self.seek_to_time(self.start_time());
                    } else {
                        self.state = PlaybackState::Stopped;
                        // Reset to beginning so we can play again easily
                        self.current_position = 0;
                        self.virtual_start_time = None;
                        self.real_start_time = None;
                    }
                }
            }
        }
    }

    /// Get a sample of messages from the start of the log for discovery (e.g. finding sensor IDs).
    /// Returns the first `max_messages` messages, or all if fewer.
    pub fn get_discovery_sample(&self, max_messages: usize) -> &[CanMessage] {
        let end = max_messages.min(self.messages.len());
        &self.messages[..end]
    }

    /// Get messages visible in the current time window
    pub fn get_window(&self, before: Duration, after: Duration) -> &[CanMessage] {
        if let Some(current) = self.current_time() {
            let start = current - before;
            let end = current + after;

            let start_idx = self.messages
                .binary_search_by(|msg| msg.timestamp.cmp(&start))
                .unwrap_or_else(|pos| pos);

            let end_idx = self.messages
                .binary_search_by(|msg| msg.timestamp.cmp(&end))
                .unwrap_or_else(|pos| pos);

            &self.messages[start_idx..end_idx]
        } else {
            &[]
        }
    }
}
