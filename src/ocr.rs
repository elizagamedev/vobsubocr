use std::{io::Cursor, str::Utf8Error};

use crate::{opt::Opt, preprocessor::PreprocessedVobSubtitle};
use image::{
    codecs::pnm::{PnmSubtype, SampleEncoding},
    DynamicImage, GrayImage,
};
use leptess::{
    leptonica::PixError,
    tesseract::{TessInitError, TessSetVariableError},
    LepTess, Variable,
};
use rayon::prelude::*;
use scoped_tls_hkt::scoped_thread_local;
use snafu::{ResultExt, Snafu};
use subparse::timetypes::TimeSpan;

scoped_thread_local!(static mut TESSERACT: Option<TesseractWrapper>);

#[derive(Debug, Snafu)]
pub enum Error {
    #[snafu(display("Could not build tesseract thread pool: {}", source))]
    BuildThreadPool { source: rayon::ThreadPoolBuildError },

    #[snafu(display("Could not initialize tesseract {}", source))]
    Initialize { source: TessInitError },

    #[snafu(display("Could not set tesseract variable: {}", source))]
    SetVariable { source: TessSetVariableError },

    #[snafu(display("Could not write image to memory: {}", source))]
    WriteImage { source: image::ImageError },

    #[snafu(display("Could not set tesseract image: {}", source))]
    SetImage { source: PixError },

    #[snafu(display("Could not get tesseract text: {}", source))]
    GetText { source: Utf8Error },

    #[snafu(display("Tesseract not initialized"))]
    TesseractNotInitialized,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

pub fn process(
    vobsubs: Vec<PreprocessedVobSubtitle>,
    opt: &Opt,
) -> Result<Vec<Result<(TimeSpan, String)>>> {
    std::env::set_var("OMP_THREAD_LIMIT", "1");
    Ok(rayon::ThreadPoolBuilder::new()
        .build_scoped(
            |thread| {
                let mut tesseract = None;
                TESSERACT.set(&mut tesseract, || thread.run())
            },
            |pool| {
                pool.install(|| {
                    vobsubs
                        .into_par_iter()
                        .map(|vobsub| {
                            let text = vobsub
                                .images
                                .into_iter()
                                .map(|image| {
                                    TESSERACT.with(|maybe_tesseract| {
                                        let tesseract = match maybe_tesseract {
                                            Some(tesseract) => tesseract,
                                            None => {
                                                let tesseract = TesseractWrapper::new(
                                                    opt.tessdata_dir.as_deref(),
                                                    &opt.lang,
                                                    &opt.config,
                                                )?;
                                                maybe_tesseract.insert(tesseract)
                                            }
                                        };
                                        tesseract.set_image(image, opt.dpi)?;
                                        Ok(tesseract.get_text()?)
                                    })
                                })
                                .collect::<Result<String>>()?;
                            Ok((vobsub.time_span, text))
                        })
                        .collect::<Vec<Result<(TimeSpan, String)>>>()
                })
            },
        )
        .context(BuildThreadPoolSnafu {})?)
}

struct TesseractWrapper {
    leptess: LepTess,
}

impl TesseractWrapper {
    fn new(
        datapath: Option<&str>,
        language: impl AsRef<str>,
        config: &[(Variable, String)],
    ) -> Result<Self> {
        let mut leptess = LepTess::new(datapath, language.as_ref()).context(InitializeSnafu {})?;
        // Disable learning by default, though a user could re-enable this
        // option with `-c`. We turn this off since we are are multithreading,
        // so this option would result in non-deterministic output.
        leptess
            .set_variable(leptess::Variable::ClassifyEnableLearning, "0")
            .context(SetVariableSnafu {})?;
        // 7 is PSM_SINGLE_LINE. We have preprocessed the input into individual
        // lines, and telling Tesseract this fact greatly improves accuracy.
        leptess
            .set_variable(leptess::Variable::TesseditPagesegMode, "7")
            .context(SetVariableSnafu {})?;
        // Add user options.
        for (key, value) in config {
            leptess
                .set_variable(*key, value)
                .context(SetVariableSnafu {})?;
        }
        Ok(Self { leptess })
    }

    /// Set the tesseract image to the given image's contents.
    fn set_image(&mut self, image: GrayImage, dpi: i32) -> Result<()> {
        let mut bytes: Cursor<Vec<u8>> = Cursor::new(Vec::new());
        DynamicImage::ImageLuma8(image)
            .write_to(
                &mut bytes,
                image::ImageOutputFormat::Pnm(PnmSubtype::Graymap(SampleEncoding::Binary)),
            )
            .context(WriteImageSnafu {})?;
        self.leptess
            .set_image_from_mem(bytes.get_ref())
            .context(SetImageSnafu {})?;
        self.leptess.set_source_resolution(dpi);
        Ok(())
    }

    /// Get text.
    fn get_text(&mut self) -> Result<String> {
        Ok(self.leptess.get_utf8_text().context(GetTextSnafu {})?)
    }
}
