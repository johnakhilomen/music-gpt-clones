/// Integration between extended audio generation and MusicGPT backend

use std::collections::VecDeque;
use std::sync::Arc;

use crate::audio::extended_generation::{
    ExtendedAudioGenerator, ExtendedGenerationConfig, SegmentGenerator,
};
use crate::backend::audio_generation_backend::JobProcessor;

/// Adapter that wraps a JobProcessor to work as a SegmentGenerator
pub struct MusicGPTSegmentGenerator {
    processor: Arc<dyn JobProcessor>,
}

impl MusicGPTSegmentGenerator {
    pub fn new(processor: Arc<dyn JobProcessor>) -> Self {
        Self { processor }
    }
}

impl SegmentGenerator for MusicGPTSegmentGenerator {
    fn generate_segment(
        &self,
        prompt: &str,
        duration: usize,
        segment_index: usize,
        on_progress: Box<dyn Fn(f32) + Send + Sync>,
    ) -> Result<VecDeque<f32>, String> {
        // Cap duration at 30 seconds (model limitation)
        let safe_duration = duration.min(30);
        
        let result = self.processor.process(
            prompt,
            safe_duration,
            Box::new({
                move |elapsed, total| {
                    on_progress(elapsed / total);
                    false // Don't abort
                }
            }),
        );

        result.map_err(|e| format!("Segment {} generation failed: {}", segment_index, e))
    }
}

/// Extended job processor that generates longer audio by stitching segments
pub struct ExtendedJobProcessor {
    base_processor: Arc<dyn JobProcessor>,
    generator: ExtendedAudioGenerator,
}

impl ExtendedJobProcessor {
    pub fn new(
        base_processor: Arc<dyn JobProcessor>,
        config: ExtendedGenerationConfig,
        sample_rate: usize,
    ) -> Result<Self, String> {
        let generator = ExtendedAudioGenerator::new(config, sample_rate)?;
        Ok(Self {
            base_processor,
            generator,
        })
    }

    /// Generate extended audio using the configured strategy
    pub fn generate_extended(
        &self,
        prompt: &str,
        on_progress: Box<dyn Fn(f32, f32) -> bool + Send + Sync + 'static>,
    ) -> ort::Result<VecDeque<f32>> {
        let segment_gen = Arc::new(MusicGPTSegmentGenerator::new(self.base_processor.clone()));
        let on_progress = Arc::new(on_progress);
        
        self.generator
            .generate(
                segment_gen,
                prompt,
                Arc::new(move |progress| {
                    (*on_progress)(progress, 1.0);
                }),
            )
            .map_err(|e| ort::Error::new(e))
    }
}

impl JobProcessor for ExtendedJobProcessor {
    fn process(
        &self,
        prompt: &str,
        secs: usize,
        on_progress: Box<dyn Fn(f32, f32) -> bool + Sync + Send + 'static>,
    ) -> ort::Result<VecDeque<f32>> {
        // If requested duration is <= 30 seconds, use base processor
        if secs <= 30 {
            return self.base_processor.process(prompt, secs, on_progress);
        }

        // Otherwise, use extended generation
        self.generate_extended(prompt, on_progress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    struct DummyProcessor;

    impl JobProcessor for DummyProcessor {
        fn process(
            &self,
            _prompt: &str,
            secs: usize,
            on_progress: Box<dyn Fn(f32, f32) -> bool + Sync + Send + 'static>,
        ) -> ort::Result<VecDeque<f32>> {
            let samples = secs * 1000; // Simulate 1000 samples/sec
            for i in 0..10 {
                let progress = (i + 1) as f32 / 10.0;
                if on_progress(progress, 1.0) {
                    return Err(ort::Error::new("Aborted"));
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(VecDeque::from(vec![0.5; samples]))
        }
    }

    #[test]
    fn test_short_duration_uses_base_processor() {
        let config = ExtendedGenerationConfig {
            target_duration: 120,
            segment_duration: 28,
            overlap_duration: 4,
            crossfade_duration: 2.0,
        };

        let extended = ExtendedJobProcessor::new(Arc::new(DummyProcessor), config, 1000).unwrap();

        let result = extended.process("test", 20, Box::new(|_, _| false));
        assert!(result.is_ok());
        assert_eq!(result.unwrap().len(), 20_000);
    }

    #[test]
    fn test_long_duration_uses_extended_generation() {
        let config = ExtendedGenerationConfig {
            target_duration: 60,
            segment_duration: 28,
            overlap_duration: 4,
            crossfade_duration: 2.0,
        };

        let extended = ExtendedJobProcessor::new(Arc::new(DummyProcessor), config, 1000).unwrap();

        let result = extended.process("test", 60, Box::new(|_, _| false));
        assert!(result.is_ok());
        
        let audio = result.unwrap();
        // Should generate approximately 60 seconds worth
        assert!(audio.len() >= 55_000 && audio.len() <= 65_000);
    }
}
