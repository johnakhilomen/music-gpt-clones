/// Extended audio generation module for creating music longer than 30 seconds
/// Uses overlapping window technique with crossfading

use std::collections::VecDeque;
use std::sync::Arc;
use tracing::info;

/// Configuration for extended audio generation
#[derive(Clone, Debug)]
pub struct ExtendedGenerationConfig {
    /// Target duration in seconds
    pub target_duration: usize,
    /// Duration of each segment (max 30 seconds due to model constraints)
    pub segment_duration: usize,
    /// Overlap duration between segments for smooth transitions (in seconds)
    pub overlap_duration: usize,
    /// Crossfade duration for blending segments (in seconds)
    pub crossfade_duration: f32,
}

impl Default for ExtendedGenerationConfig {
    fn default() -> Self {
        Self {
            target_duration: 240, // 4 minutes
            segment_duration: 28, // Leave buffer below 30s
            overlap_duration: 4,
            crossfade_duration: 2.0,
        }
    }
}

impl ExtendedGenerationConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.segment_duration > 30 {
            return Err("Segment duration cannot exceed 30 seconds due to model limitations".to_string());
        }
        if self.overlap_duration >= self.segment_duration {
            return Err("Overlap duration must be less than segment duration".to_string());
        }
        if self.crossfade_duration > self.overlap_duration as f32 {
            return Err("Crossfade duration must be less than or equal to overlap duration".to_string());
        }
        Ok(())
    }

    pub fn num_segments(&self) -> usize {
        let effective_segment = self.segment_duration - self.overlap_duration;
        ((self.target_duration + effective_segment - 1) / effective_segment).max(1)
    }
}

///Trait for generating audio segments
pub trait SegmentGenerator: Send + Sync {
    fn generate_segment(
        &self,
        prompt: &str,
        duration: usize,
        segment_index: usize,
        on_progress: Box<dyn Fn(f32) + Send + Sync>,
    ) -> Result<VecDeque<f32>, String>;
}

/// Extended audio generator that creates long-form music
pub struct ExtendedAudioGenerator {
    config: ExtendedGenerationConfig,
    sample_rate: usize,
}

impl ExtendedAudioGenerator {
    pub fn new(config: ExtendedGenerationConfig, sample_rate: usize) -> Result<Self, String> {
        config.validate()?;
        Ok(Self { config, sample_rate })
    }

    /// Generate extended audio by creating and blending multiple segments
    pub fn generate<G: SegmentGenerator>(
        &self,
        generator: Arc<G>,
        prompt: &str,
        on_progress: Arc<dyn Fn(f32) + Send + Sync>,
    ) -> Result<VecDeque<f32>, String> {
        let num_segments = self.config.num_segments();
        info!("Generating {} segments for {}-second audio", num_segments, self.config.target_duration);

        let mut final_audio = VecDeque::new();
        
        for i in 0..num_segments {
            let segment_progress = i as f32 / num_segments as f32;
            
            // Create varied prompts for different segments to maintain interest
            let segment_prompt = self.create_segment_prompt(prompt, i, num_segments);
            
            info!("Generating segment {}/{}: {}", i + 1, num_segments, segment_prompt);
            
            // Generate segment with progress callback
            let on_prog_clone = on_progress.clone();
            let segment_audio = generator.generate_segment(
                &segment_prompt,
                self.config.segment_duration,
                i,
                Box::new(move |seg_progress| {
                    let total_progress = segment_progress + (seg_progress / num_segments as f32);
                    on_prog_clone(total_progress);
                }),
            )?;

            if i == 0 {
                // First segment: add everything
                final_audio.extend(segment_audio);
            } else {
                // Subsequent segments: crossfade with previous audio
                let overlap_samples = (self.config.overlap_duration as f32 * self.sample_rate as f32) as usize;
                let crossfade_samples = (self.config.crossfade_duration * self.sample_rate as f32) as usize;
                
                final_audio = self.crossfade_segments(
                    final_audio,
                    segment_audio,
                    overlap_samples,
                    crossfade_samples,
                );
            }
        }

        // Trim to exact target duration
        let target_samples = self.config.target_duration * self.sample_rate;
        final_audio.truncate(target_samples);

        info!("Extended audio generation complete: {} samples", final_audio.len());
        Ok(final_audio)
    }

    /// Create contextual prompts for different segments
    fn create_segment_prompt(&self, base_prompt: &str, segment_index: usize, total_segments: usize) -> String {
        // Add variation keywords based on position in the piece
        match segment_index {
            0 => format!("{} (introduction, opening)", base_prompt),
            i if i == total_segments - 1 => format!("{} (conclusion, ending, outro)", base_prompt),
            i if i == total_segments / 2 => format!("{} (bridge, development, variation)", base_prompt),
            i if i < total_segments / 3 => format!("{} (building, developing)", base_prompt),
            _ => base_prompt.to_string(),
        }
    }

    /// Crossfade two audio segments with overlap
    fn crossfade_segments(
        &self,
        mut segment1: VecDeque<f32>,
        segment2: VecDeque<f32>,
        overlap_samples: usize,
        crossfade_samples: usize,
    ) -> VecDeque<f32> {
        if segment1.len() < overlap_samples {
            // Not enough samples to overlap, just concatenate
            segment1.extend(segment2);
            return segment1;
        }

        // Calculate where crossfade starts
        let crossfade_start = segment1.len().saturating_sub(crossfade_samples);
        
        // Apply linear crossfade
        for i in 0..crossfade_samples.min(segment2.len()) {
            let fade_position = i as f32 / crossfade_samples as f32;
            let idx = crossfade_start + i;
            
            if idx < segment1.len() && i < segment2.len() {
                // Linear crossfade: fade out segment1, fade in segment2
                let fade_out = 1.0 - fade_position;
                let fade_in = fade_position;
                
                segment1[idx] = segment1[idx] * fade_out + segment2[i] * fade_in;
            }
        }

        // Remove the overlapped portion and append the rest
        let skip_samples = crossfade_samples.min(segment2.len());
        segment1.extend(segment2.iter().skip(skip_samples));
        
        segment1
    }

    /// Apply smoothing to avoid clicks and pops
    pub fn apply_smoothing(audio: &mut VecDeque<f32>, window_size: usize) {
        if audio.len() < window_size * 2 {
            return;
        }

        // Smooth the beginning
        for i in 0..window_size {
            let factor = i as f32 / window_size as f32;
            audio[i] *= factor;
        }

        // Smooth the end
        let len = audio.len();
        for i in 0..window_size {
            let factor = (window_size - i) as f32 / window_size as f32;
            audio[len - 1 - i] *= factor;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct DummyGenerator;
    
    impl SegmentGenerator for DummyGenerator {
        fn generate_segment(
            &self,
            _prompt: &str,
            duration: usize,
            _segment_index: usize,
            _on_progress: Box<dyn Fn(f32) + Send>,
        ) -> Result<VecDeque<f32>, String> {
            // Generate dummy audio (1 second = 1000 samples for test)
            let samples = duration * 1000;
            Ok(VecDeque::from(vec![0.5; samples]))
        }
    }

    #[test]
    fn test_config_validation() {
        let config = ExtendedGenerationConfig {
            segment_duration: 35,
            ..Default::default()
        };
        assert!(config.validate().is_err());

        let config = ExtendedGenerationConfig::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_num_segments() {
        let config = ExtendedGenerationConfig {
            target_duration: 240,
            segment_duration: 28,
            overlap_duration: 4,
            ..Default::default()
        };
        
        // Effective segment length is 28 - 4 = 24 seconds
        // 240 / 24 = 10 segments
        assert_eq!(config.num_segments(), 10);
    }

    #[test]
    fn test_extended_generation() {
        let config = ExtendedGenerationConfig {
            target_duration: 60,
            segment_duration: 28,
            overlap_duration: 4,
            crossfade_duration: 2.0,
        };

        let generator = ExtendedAudioGenerator::new(config, 1000).unwrap();
        let result = generator.generate(
            Arc::new(DummyGenerator),
            "test prompt",
            Box::new(|_| {}),
        );

        assert!(result.is_ok());
        let audio = result.unwrap();
        
        // Should be exactly 60 seconds * 1000 samples/sec = 60000 samples
        assert_eq!(audio.len(), 60_000);
    }

    #[test]
    fn test_crossfade() {
        let config = ExtendedGenerationConfig::default();
        let generator = ExtendedAudioGenerator::new(config, 1000).unwrap();

        let segment1 = VecDeque::from(vec![1.0; 10000]);
        let segment2 = VecDeque::from(vec![0.0; 10000]);

        let result = generator.crossfade_segments(segment1, segment2, 2000, 1000);
        
        // Check that crossfade happened
        assert!(result.len() > 10000);
        
        // Values in crossfade region should be between 0.0 and 1.0
        let crossfade_start = 10000 - 1000;
        for i in 0..1000 {
            let val = result[crossfade_start + i];
            assert!(val >= 0.0 && val <= 1.0, "Value at crossfade should be blended: {}", val);
        }
    }
}
