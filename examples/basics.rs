use std::io::Write;

const SAMPLE_RATE: usize = 24000;
const CHUNK_SIZE: usize = 1920;
const FREQUENCY: usize = 440;

fn main() -> anyhow::Result<()> {
    let mut encoder = kaudio::ogg_opus::Encoder::new(SAMPLE_RATE)?;
    let freq_in_samples = SAMPLE_RATE / FREQUENCY;
    let mut file = std::fs::File::create("out.ogg")?;
    file.write_all(encoder.header_data())?;
    for i in 0..100 {
        let start_index = i * CHUNK_SIZE;
        let pcm: Vec<_> = (start_index..start_index + CHUNK_SIZE)
            .map(|v| {
                ((v % freq_in_samples) as f64 / freq_in_samples as f64 * std::f64::consts::PI * 2.)
                    .sin() as f32
            })
            .collect();
        let bytes = encoder.encode_page(&pcm)?;
        file.write_all(&bytes)?;
    }

    Ok(())
}
