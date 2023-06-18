use std::collections::VecDeque;

use itertools::Itertools;
use rodio::Source;
use soundtouch::SoundTouch;

pub fn pitch_shift<I: Source<Item = f32>>(mut input: I, semitones: i32) -> PitchShift<I> {
    let channels = input.channels();
    let mut st = SoundTouch::new(channels, input.sample_rate());
    st.set_pitch_semi_tones(semitones);
    let min_samples = st.get_setting(soundtouch::settings::SETTING_NOMINAL_INPUT_SEQUENCE) as usize
        * channels as usize;
    let initial_latency =
        st.get_setting(soundtouch::settings::SETTING_INITIAL_LATENCY) as usize * channels as usize;
    let mut out_buffer = VecDeque::new();
    out_buffer.resize(initial_latency, 0.0);
    out_buffer.make_contiguous();
    let mut initial_input = input.by_ref().take(initial_latency).collect_vec();
    st.put_samples(&initial_input);
    let read = st.read_samples(out_buffer.as_mut_slices().0);
    out_buffer.truncate(read as usize);
    initial_input.clear();
    PitchShift {
        input,
        min_samples,
        soundtouch: st,
        out_buffer,
        in_buffer: initial_input,
    }
}

pub struct PitchShift<I: Source<Item = f32>> {
    input: I,
    soundtouch: SoundTouch,
    min_samples: usize,
    out_buffer: VecDeque<f32>,
    in_buffer: Vec<f32>,
}

impl<I> Iterator for PitchShift<I>
where
    I: Source<Item = f32>,
{
    type Item = I::Item;

    fn next(&mut self) -> Option<Self::Item> {
        if self.out_buffer.is_empty() {
            self.in_buffer.clear();
            self.input
                .by_ref()
                .take(self.min_samples)
                .for_each(|x| self.in_buffer.push(x));

            self.soundtouch.put_samples(&self.in_buffer);

            self.out_buffer.resize(self.min_samples, 0.0);
            self.out_buffer.make_contiguous();

            let read = self
                .soundtouch
                .read_samples(self.out_buffer.as_mut_slices().0);

            self.out_buffer
                .truncate((read * self.input.channels() as u32) as usize)
        }

        self.out_buffer.pop_front()
    }
}

impl<I> Source for PitchShift<I>
where
    I: Source<Item = f32>,
{
    fn current_frame_len(&self) -> Option<usize> {
        Some(self.min_samples)
    }

    fn channels(&self) -> u16 {
        self.input.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.input.sample_rate()
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        self.input.total_duration()
    }
}
