#![doc = include_str!("../README.md")]

mod ocr;
mod opt;
mod preprocessor;

use crate::opt::Opt;
use clap::Parser;
use log::{warn, LevelFilter};
use snafu::{ErrorCompat, ResultExt, Snafu};
use std::{
    fs::File,
    io::{self, Write},
    path::PathBuf,
};
use subparse::{timetypes::TimeSpan, SrtFile, SubtitleFile};

#[derive(Debug, Snafu)]
enum Error {
    #[snafu(display("Could not parse VOB subtitles from {}: {}", filename.display(), source))]
    ReadSubtitles {
        filename: PathBuf,
        source: vobsub::Error,
    },

    #[snafu(display("Could not perform OCR on subtitles: {}", source))]
    Ocr { source: ocr::Error },

    #[snafu(display("Could not generate SRT file: {}", message))]
    GenerateSrt { message: String },

    #[snafu(display("Could not write SRT file {}: {}", filename.display(), source))]
    WriteSrt {
        filename: PathBuf,
        source: io::Error,
    },

    #[snafu(display("Could not write image dump file {}: {}", filename, source))]
    DumpImage {
        filename: String,
        source: image::ImageError,
    },
}

type Result<T, E = Error> = std::result::Result<T, E>;

fn run(opt: Opt) -> Result<i32> {
    let vobsubs = preprocessor::preprocess_subtitles(&opt).context(ReadSubtitlesSnafu {
        filename: opt.input.clone(),
    })?;

    // Dump images if requested.
    if opt.dump {
        for (i, sub) in vobsubs.iter().enumerate() {
            for (j, image) in sub.images.iter().enumerate() {
                let filename = format!("{:06}-{:02}.png", i, j);
                image.save(&filename).context(DumpImageSnafu { filename })?;
            }
        }
    }

    let subtitles = ocr::process(vobsubs, &opt).context(OcrSnafu {})?;

    // Log errors and remove bad results.
    let mut return_code = 0;
    let subtitles: Vec<(TimeSpan, String)> = subtitles
        .into_iter()
        .filter_map(|maybe_subtitle| match maybe_subtitle {
            Ok(subtitle) => Some(subtitle),
            Err(e) => {
                warn!("Error while running OCR on subtitle image: {}", e);
                return_code = 1;
                None
            }
        })
        .collect();

    // Create subtitle file.
    let subtitles = SubtitleFile::SubRipFile(SrtFile::create(subtitles).map_err(|e| {
        GenerateSrtSnafu {
            message: e.to_string(),
        }
        .build()
    })?);
    let subtitle_data = subtitles.to_data().map_err(|e| {
        GenerateSrtSnafu {
            message: e.to_string(),
        }
        .build()
    })?;

    match opt.output {
        Some(output) => {
            // Write to file.
            let mut subtitle_file = File::create(&output).context(WriteSrtSnafu {
                filename: output.clone(),
            })?;
            subtitle_file
                .write_all(&subtitle_data)
                .context(WriteSrtSnafu { filename: output })?;
        }
        None => {
            // Write to stdout.
            io::stdout()
                .write_all(&subtitle_data)
                .context(WriteSrtSnafu {
                    filename: "<stdout>",
                })?;
        }
    }

    Ok(return_code)
}

fn main() {
    simple_logger::SimpleLogger::new()
        .without_timestamps()
        .with_level(LevelFilter::Warn)
        .env()
        .init()
        .unwrap();
    let code = match run(Opt::parse()) {
        Ok(rc) => rc,
        Err(e) => {
            eprintln!("An error occurred: {}", e);
            if let Some(backtrace) = ErrorCompat::backtrace(&e) {
                println!("{}", backtrace);
            }
            1
        }
    };
    std::process::exit(code);
}
