/*
 * Golden-vector generator for the itsalive ADV7513 tables.
 *
 * hdmi_config_init(), hdmi_config_audio(), hdmi_config_set_csc() and
 * hdmi_config_set_mode() below are transcribed verbatim from Main_MiSTer's
 * video.cpp at upstream commit 6cda9cc -- lines 1462-1615, 1417-1460,
 * 1180-1400 and 1680-1727 respectively.  The tables, the float chain and the
 * cfg-dependent expressions are the upstream text, character for character.
 * The only edits are:
 *
 *   - the i2c plumbing is gone: hdmi_main_fd, i2c_open() and the printf()
 *     error paths are dropped, and each loop body's
 *     `i2c_smbus_write_byte_data(hdmi_main_fd, t[i], t[i + 1])` becomes
 *     `row(t[i], t[i + 1])`, which appends the pair to a buffer.  The loops
 *     themselves, which are what fix the order, are untouched;
 *   - `cfg` is reduced to the dozen fields these four functions read, set to
 *     the values cfg_parse() (cfg.cpp:592-613) leaves in place when MiSTer.ini
 *     is absent -- that is the configuration itsalive ships under;
 *   - each function records where its rows begin, so main() can slice the one
 *     buffer back into three tables without reordering anything;
 *   - hdmi_has_int() is replaced by the constant HDMI_HAS_INT (see below);
 *   - PROFILE_FUNCTION(), hdmi_power/hdmi_need_init and the EDID/SPD sub-map
 *     opens are dropped; none of them contributes a register write.
 *
 * Run adv7513-gen.sh to compile this and rewrite adv7513.json in place.
 * Nothing in the Rust build compiles this file; the JSON is checked in and the
 * Rust test only reads it.
 *
 * Both projects are GPL-3.0-or-later.
 */

#include <math.h>
#include <stdint.h>
#include <stdio.h>
#include <string.h>

#include "mat4x4.h"

/*
 * video.cpp:1465, `uint8_t int0 = hdmi_has_int() ? 0xC0 : 0x00;`.
 *
 * hdmi_has_int() (video.cpp:1406-1415) is a run-time query to the fabric
 * (`spi_uio_cmd(UIO_HDMI_INT)`): 1 on a core that routes the ADV7513's
 * interrupt pin, 0 on one that does not.  itsalive never arms or services
 * those interrupts (docs/ARCHITECTURE.md section 4, "Not done"), so it never
 * makes the query and always writes the 0x00 arm mask.  Pinning this to 0 is
 * therefore the generator modelling itsalive's one declared divergence from
 * Main, not a transcription choice -- and setting it to 1 here reproduces
 * Main's other branch byte for byte, which is how a reviewer checks that the
 * divergence really is confined to register 0x94.
 */
#define HDMI_HAS_INT 0

/* The slice of cfg_t (cfg.h:19-100) these four functions read. */
struct cfg_t
{
	uint8_t hdmi_audio_96k;
	uint8_t dvi_mode;
	uint8_t hdmi_limited;
	uint8_t direct_video;
	uint8_t hdmi_game_mode;
	uint8_t video_brightness;
	uint8_t video_contrast;
	uint8_t video_saturation;
	uint16_t video_hue;
	char video_gain_offset[256];
	uint8_t hdr;
	char vga_mode_int;
};

/*
 * cfg_parse() (cfg.cpp:592-613) memsets cfg to zero and then overrides a
 * handful of fields.  With no MiSTer.ini to parse, what it leaves behind for
 * the fields above is: zero everywhere, except dvi_mode = 2 (cfg.cpp:603),
 * video_brightness = 50 (:609), video_contrast = 50 (:610),
 * video_saturation = 100 (:611) and video_gain_offset = "1, 0, 1, 0, 1, 0"
 * (:612).
 */
static cfg_t cfg;

static void cfg_defaults(void)
{
	memset(&cfg, 0, sizeof(cfg));		/* cfg.cpp:594 */
	cfg.dvi_mode = 2;			/* cfg.cpp:603 */
	cfg.video_brightness = 50;		/* cfg.cpp:609 */
	cfg.video_contrast = 50;		/* cfg.cpp:610 */
	cfg.video_saturation = 100;		/* cfg.cpp:611 */
	strcpy(cfg.video_gain_offset, "1, 0, 1, 0, 1, 0");	/* cfg.cpp:612 */
}

/* Added: the i2c bus, as a buffer. */
static uint8_t rows[512][2];
static unsigned nrows;
static unsigned init_begin, audio_begin, csc_begin;

static void row(uint8_t addr, uint8_t value)
{
	rows[nrows][0] = addr;
	rows[nrows][1] = value;
	nrows++;
}

/* video.cpp:1417-1460 */
static void hdmi_config_audio()
{
	audio_begin = nrows;	/* Added. */

	// address, value
	uint8_t init_data[] = {

		0xAF, (uint8_t)(0b00000100	// [7]=0 HDCP Disabled.
								// [6:5] must be b00!
								// [4]=0 Current frame is unencrypted
								// [3:2] must be b01!
			| ((cfg.dvi_mode == 1) ? 0b00 : 0b10)),	 //	[1]=1 HDMI Mode.
								// [0] must be b0!

		// (Audio stuff on Programming Guide, Page 66)...
		0x0A, 0b00000000,		// [6:4] Audio Select. b000 = I2S.
								// [3:2] Audio Mode. (HBR stuff, leave at 00!).

		0x0B, 0b00001110,		//

		0x0C, 0b00000100,		// [7] 0 = Use sampling rate from I2S stream.   1 = Use samp rate from I2C Register.
								// [6] 0 = Use Channel Status bits from stream. 1 = Use Channel Status bits from I2C register.
								// [2] 1 = I2S0 Enable.
								// [1:0] I2S Format: 00 = Standard. 01 = Right Justified. 10 = Left Justified. 11 = AES.

		0x0D, 0b00010000,		// [4:0] I2S Bit (Word) Width for Right-Justified.
		0x14, 0b00000010,		// [3:0] Audio Word Length. b0010 = 16 bits.
		0x15, (uint8_t)((cfg.hdmi_audio_96k ? 0x80 : 0x00) | 0b0100000),	// I2S Sampling Rate [7:4]. b0000 = (44.1KHz). b0010 = 48KHz.
								// Input ID [3:1] b000 (0) = 24-bit RGB 444 or YCrCb 444 with Separate Syncs.

		// Audio Clock Config
		0x01, 0x00,				//
		0x02, (uint8_t)(cfg.hdmi_audio_96k ? 0x30 : 0x18),	// Set N Value 12288/6144
		0x03, 0x00,				//

		0x07, 0x01,				//
		0x08, 0x22,				// Set CTS Value 74250
		0x09, 0x0A,				//
	};

	for (unsigned i = 0; i < sizeof(init_data); i += 2)
	{
		row(init_data[i], init_data[i + 1]);
	}
}

/* video.cpp:1180-1400 */
static void hdmi_config_set_csc()
{
	csc_begin = nrows;	/* Added. */

	// default color conversion matrices
	// for the original hexadecimal versions please refer
	// to the ADV7513 programming guide section 4.3.7

	// no transformation, so use identity matrix
	float hdmi_full_coeffs[] = {
		1.0f, 0.0f, 0.0f, 0.0f,
		0.0f, 1.0f, 0.0f, 0.0f,
		0.0f, 0.0f, 1.0f, 0.0f,
		0.0f, 0.0f, 0.0f, 1.0f
	};

	float hdmi_limited_1_coeffs[] = {
		0.8583984375f, 0.0f, 0.0f, 0.06250f,
		0.0f, 0.8583984375f, 0.0f, 0.06250f,
		0.0f, 0.0f, 0.8583984375f, 0.06250f,
		0.0f, 0.0f, 0.0f, 1.0f
	};

	float hdmi_limited_2_coeffs[] = {
		0.93701171875f, 0.0f, 0.0f, 0.06250f,
		0.0f, 0.93701171875f, 0.0f, 0.06250f,
		0.0f, 0.0f, 0.93701171875f, 0.06250f,
		0.0f, 0.0f, 0.0f, 1.0f
	};

	float hdr_dcip3_coeffs[] = {
		0.8225f, 0.1774f, 0.0000f, 0.0f,
		0.0332f, 0.9669f, 0.0000f, 0.0f,
		0.0171f, 0.0724f, 0.9108f, 0.0f,
		0.0f, 0.0f, 0.0f, 1.0f
	};

	const float pi = float(M_PI);

	int ypbpr = (cfg.vga_mode_int == 1) && (cfg.direct_video == 1);

	// out-of-scope defines, not used with ypbpr
	int16_t csc_int16[12];
	int hdmi_limited_1 = cfg.hdmi_limited & 1;
	int hdmi_limited_2 = cfg.hdmi_limited & 2;

	if (!ypbpr)
	{
		// select the base CSC
		int hdr = cfg.hdr;

		mat4x4 coeffs = hdr == 2 ? hdr_dcip3_coeffs : hdmi_full_coeffs;
		mat4x4 csc(coeffs);

		// apply color controls
		float brightness = (((cfg.video_brightness / 100.0f) - 0.5f)); // [-0.5 .. 0.5]
		float contrast = ((cfg.video_contrast / 100.0f) - 0.5f) * 2.0f + 1.0f; // [0 .. 2]
		float saturation = ((cfg.video_saturation / 100.0f)); // [0 .. 1]
		float hue = (cfg.video_hue * pi / 180.0f);

		char* gain_offset = cfg.video_gain_offset;

		// we have to parse these
		float gain_red = 1;
		float gain_green = 1;
		float gain_blue = 1;
		float off_red = 0;
		float off_green = 0;
		float off_blue = 0;

		size_t target = 0;
		float* targets[6] = { &gain_red, &off_red, &gain_green, &off_green, &gain_blue, &off_blue };

		for (size_t i = 0; i < strlen(gain_offset) && target < 6; i++)
		{
			// skip whitespace
			if (gain_offset[i] == ' ' || gain_offset[i] == ',')
				continue;

			int numRead = 0;
			int match = sscanf(gain_offset + i, "%f%n", targets[target], &numRead);

			i += numRead > 0 ? numRead - 1 : 0;

			if (match == 1)
				target++;
		}

		// first apply hue matrix, because it does not touch luminance
		float cos_hue = cos(hue);
		float sin_hue = sin(hue);
		float lr = 0.213f;
		float lg = 0.715f;
		float lb = 0.072f;
		float ca = 0.143f;
		float cb = 0.140f;
		float cc = 0.283f;

		mat4x4 mat_hue;
		mat_hue.setIdentity();

		mat_hue.m11 = lr + cos_hue * (1 - lr) + sin_hue * (-lr);
		mat_hue.m12 = lg + cos_hue * (-lg) + sin_hue * (-lg);
		mat_hue.m13 = lb + cos_hue * (-lb) + sin_hue * (1 - lb);

		mat_hue.m21 = lr + cos_hue * (-lr) + sin_hue * (ca);
		mat_hue.m22 = lg + cos_hue * (1 - lg) + sin_hue * (cb);
		mat_hue.m23 = lb + cos_hue * (-lb) + sin_hue * (cc);

		mat_hue.m31 = lr + cos_hue * (-lr) + sin_hue * (-(1 - lr));
		mat_hue.m32 = lg + cos_hue * (-lg) + sin_hue * (lg);
		mat_hue.m33 = lb + cos_hue * (1 - lb) + sin_hue * (lb);

		csc = csc * mat_hue;

		// now saturation
		float s = saturation;
		float sr = (1.0f - s) * .3086f;
		float sg = (1.0f - s) * .6094f;
		float sb = (1.0f - s) * .0920f;

		float mat_saturation[] = {
			sr + s, sg, sb, 0,
			sr, sg + s, sb, 0,
			sr, sg, sb + s, 0,
			0, 0, 0, 1.0f
		};

		csc = csc * mat4x4(mat_saturation);

		// now brightness and contrast
		float b = brightness;
		float c = contrast;
		float t = (1.0f - c) / 2.0f;

		float mat_brightness_contrast[] = {
			c, 0, 0, (t + b),
			0, c, 0, (t + b),
			0, 0, c, (t + b),
			0, 0, 0, 1.0f
		};

		csc = csc * mat4x4(mat_brightness_contrast);

		// gain and offset
		float rg = gain_red;
		float ro = off_red;
		float gg = gain_green;
		float go = off_green;
		float bg = gain_blue;
		float bo = off_blue;

		float mat_gain_off[] = {
			rg, 0, 0, ro,
			0, gg, 0, go,
			0, 0, bg, bo,
			0, 0, 0, 1.0f
		};

		csc = csc * mat4x4(mat_gain_off);

		// final compression
		csc.compress(2.0f);

		// make sure to retain hdmi limited range
		if (hdmi_limited_1)
			csc = csc * mat4x4(hdmi_limited_1_coeffs);
		else if (hdmi_limited_2)
			csc = csc * mat4x4(hdmi_limited_2_coeffs);

		// finally, apply a fixed multiplier to get it in
		// correct range for ADV7513 chip
		for (size_t i = 0; i < 12; i++)
		{
			csc_int16[i] = int16_t(csc.comp[i] * 2048.0f);
		}
	}
	// Clamps to reinforce limited if necessary
	// 0x100 = 16/256 * 4096 (12-bit mul)
	// 0xEB0 = 235/256 * 4096
	// 0xFFF = 4095 (12-bit max)
	uint16_t clipMin = (!ypbpr && (hdmi_limited_1 || hdmi_limited_2)) ? 0x100 : 0x000;
	uint16_t clipMax = (!ypbpr && hdmi_limited_1) ? 0xEB0 : 0xFFF;

	// pass to HDMI, use 0xA0 to set a mode of [-2 .. 2] per ADV7513 programming guide
	uint8_t csc_data[] = {
		0x18, (uint8_t)(ypbpr ? 0x86 : (0b10100000 | (((csc_int16[0] >> 8) & 0b00011111)))),  // csc Coefficients, Channel A
		0x19, (uint8_t)(ypbpr ? 0xDF : (csc_int16[0] & 0xff)),
		0x1A, (uint8_t)(ypbpr ? 0x1A : (csc_int16[1] >> 8)),
		0x1B, (uint8_t)(ypbpr ? 0x3F : (csc_int16[1] & 0xff)),
		0x1C, (uint8_t)(ypbpr ? 0x1E : (csc_int16[2] >> 8)),
		0x1D, (uint8_t)(ypbpr ? 0xE2 : (csc_int16[2] & 0xff)),
		0x1E, (uint8_t)(ypbpr ? 0x07 : (csc_int16[3] >> 8)),
		0x1F, (uint8_t)(ypbpr ? 0xE7 : (csc_int16[3] & 0xff)),

		0x20, (uint8_t)(ypbpr ? 0x04 : (csc_int16[4] >> 8)),  // csc Coefficients, Channel B
		0x21, (uint8_t)(ypbpr ? 0x1C : (csc_int16[4] & 0xff)),
		0x22, (uint8_t)(ypbpr ? 0x08 : (csc_int16[5] >> 8)),
		0x23, (uint8_t)(ypbpr ? 0x11 : (csc_int16[5] & 0xff)),
		0x24, (uint8_t)(ypbpr ? 0x01 : (csc_int16[6] >> 8)),
		0x25, (uint8_t)(ypbpr ? 0x91 : (csc_int16[6] & 0xff)),
		0x26, (uint8_t)(ypbpr ? 0x01 : (csc_int16[7] >> 8)),
		0x27, (uint8_t)(ypbpr ? 0x00 : (csc_int16[7] & 0xff)),

		0x28, (uint8_t)(ypbpr ? 0x1D : (csc_int16[8] >> 8)),  // csc Coefficients, Channel C
		0x29, (uint8_t)(ypbpr ? 0xAE : (csc_int16[8] & 0xff)),
		0x2A, (uint8_t)(ypbpr ? 0x1B : (csc_int16[9] >> 8)),
		0x2B, (uint8_t)(ypbpr ? 0x73 : (csc_int16[9] & 0xff)),
		0x2C, (uint8_t)(ypbpr ? 0x06 : (csc_int16[10] >> 8)),
		0x2D, (uint8_t)(ypbpr ? 0xDF : (csc_int16[10] & 0xff)),
		0x2E, (uint8_t)(ypbpr ? 0x07 : (csc_int16[11] >> 8)),
		0x2F, (uint8_t)(ypbpr ? 0xE7 : (csc_int16[11] & 0xff)),

		0xC0, (uint8_t)(clipMin >> 8), // HDMI limited clamps
		0xC1, (uint8_t)(clipMin & 0xff),
		0xC2, (uint8_t)(clipMax >> 8),
		0xC3, (uint8_t)(clipMax & 0xff)
	};

	for (unsigned i = 0; i < sizeof(csc_data); i += 2)
	{
		row(csc_data[i], csc_data[i + 1]);
	}
}

/* video.cpp:1462-1615 */
static void hdmi_config_init()
{
	init_begin = nrows;	/* Added. */

	int ypbpr = (cfg.vga_mode_int == 1) && (cfg.direct_video == 1);
	uint8_t int0 = HDMI_HAS_INT ? 0xC0 : 0x00; // HPD + SENSE

	// address, value
	uint8_t init_data[] = {
		0x98, 03,				// ADI required Write.

		0xD6, 0b11000000,		// [7:6] HPD Control...
								// 00 = HPD is from both HPD pin or CDC HPD
								// 01 = HPD is from CDC HPD
								// 10 = HPD is from HPD pin
								// 11 = HPD is always high

		0x41, 0x10,				// Power Down control
		0x9A, 0x70,				// ADI required Write.
		0x9C, 0x30,				// ADI required Write.
		0x9D, 0b01100001,		// [7:4] must be b0110!.
								// [3:2] b00 = Input clock not divided. b01 = Clk divided by 2. b10 = Clk divided by 4. b11 = invalid!
								// [1:0] must be b01!
		0xA2, 0xA4,				// ADI required Write.
		0xA3, 0xA4,				// ADI required Write.
		0xE0, 0xD0,				// ADI required Write.


		0x35, 0x40,
		0x36, 0xD9,
		0x37, 0x0A,
		0x38, 0x00,
		0x39, 0x2D,
		0x3A, 0x00,

		0x16, 0b00111000,		// Output Format 444 [7]=0.
								// [6] must be 0!
								// Colour Depth for Input Video data [5:4] b11 = 8-bit.
								// Input Style [3:2] b10 = Style 1 (ignored when using 444 input).
								// DDR Input Edge falling [1]=0 (not using DDR atm).
								// Output Colour Space RGB [0]=0.

		0x17, 0b01100010,		// Aspect ratio 16:9 [1]=1, 4:3 [1]=0, invert sync polarity

		0x3B, 0x80,             // Automatic pixel repetition and VIC detection
		0x3C, 0x00,

		0x48, 0b00001000,       // [6]=0 Normal bus order!
								// [5] DDR Alignment.
								// [4:3] b01 Data right justified (for YCbCr 422 input modes).

		0x49, 0xA8,				// ADI required Write.
		0x40, 0x00,
		0x4A, 0b10000000,		//Auto-Calculate SPD checksum
		0x4C, 0x00,				// ADI required Write.

		0x55, (uint8_t)(cfg.hdmi_game_mode ? 0b00010010 : 0b00010000),
								// [7] must be 0!. Set RGB444 in AVinfo Frame [6:5], Set active format [4].
								// AVI InfoFrame Valid [4].
								// Bar Info [3:2] b00 Bars invalid. b01 Bars vertical. b10 Bars horizontal. b11 Bars both.
								// Scan Info [1:0] b00 (No data). b01 TV. b10 PC. b11 None.

		0x56, (uint8_t)( 0b00001000 | (cfg.hdr ? 0xb11000000 : 0)),		// [5:4] Picture Aspect Ratio
								// [3:0] Active Portion Aspect Ratio b1000 = Same as Picture Aspect Ratio

		0x57, (uint8_t)((cfg.hdmi_game_mode ? 0x80 : 0x00)		// [7] IT Content. 0 - No. 1 - Yes (type set in register 0x59).
																// [6:4] Color space (ignored for RGB)
			| ((ypbpr || cfg.hdmi_limited) ? 0b0100 : cfg.hdr ? 0b1101000 : 0b0001000)),	// [3:2] RGB Quantization range
																// [1:0] Non-Uniform Scaled: 00 - None. 01 - Horiz. 10 - Vert. 11 - Both.

		0x59, (uint8_t)(cfg.hdmi_game_mode ? 0x30 : 0x00),		// [7:6] [YQ1 YQ0] YCC Quantization Range: b00 = Limited Range, b01 = Full Range
																// [5:4] IT Content Type b11 = Game, b00 = Graphics/None
																// [3:0] Pixel Repetition Fields b0000 = No Repetition

		0x73, 0x01,

		0x96, 0xFF,             // clear all pending interrupts
		0x94, int0,
		0xC9, 0x00,             // Clear EDID request

		0x99, 0x02,				// ADI required Write.
		0x9B, 0x18,				// ADI required Write.

		0x9F, 0x00,				// ADI required Write.

		0xA1, 0b00000000,	    // [6]=1 Monitor Sense Power Down DISabled.

		0xA4, 0x08,				// ADI required Write.
		0xA5, 0x04,				// ADI required Write.
		0xA6, 0x00,				// ADI required Write.
		0xA7, 0x00,				// ADI required Write.
		0xA8, 0x00,				// ADI required Write.
		0xA9, 0x00,				// ADI required Write.
		0xAA, 0x00,				// ADI required Write.
		0xAB, 0x40,				// ADI required Write.

		0xB9, 0x00,				// ADI required Write.

		0xBA, 0b01100000,		// [7:5] Input Clock delay...
								// b000 = -1.2ns.
								// b001 = -0.8ns.
								// b010 = -0.4ns.
								// b011 = No delay.
								// b100 = 0.4ns.
								// b101 = 0.8ns.
								// b110 = 1.2ns.
								// b111 = 1.6ns.

		0xBB, 0x00,				// ADI required Write.
		0xDE, 0x9C,				// ADI required Write.
		0xE2, 0x01,				// Power down the CEC.
		0xE4, 0x60,				// ADI required Write.
		0xFA, 0x7D,				// Nbr of times to search for good phase
	};

	for (unsigned i = 0; i < sizeof(init_data); i += 2)
	{
		row(init_data[i], init_data[i + 1]);
	}

	hdmi_config_audio();
	hdmi_config_set_csc();
}

/*
 * video.cpp:1680-1727.  The three shadow variables and the early return are
 * upstream's; keeping them is the point, because itsalive drops them and the
 * golden is where that decision is made visible.
 */
static uint8_t last_sync_invert = 0xff;
static uint8_t last_pr_flags = 0xff;
static uint8_t last_vic_mode = 0xff;

static void hdmi_invalidate_mode_cache()
{
	last_sync_invert = 0xff;
	last_pr_flags = 0xff;
	last_vic_mode = 0xff;
}

/* Added: the fields of vmode_custom_t::param that set_mode reads. */
struct vmode_param_t { uint32_t vic; uint32_t pr; uint32_t hpol; uint32_t vpol; };
struct vmode_custom_t { vmode_param_t param; };

static void hdmi_config_set_mode(vmode_custom_t *vm)
{
	const uint8_t vic_mode = (uint8_t)vm->param.vic;
	uint8_t pr_flags;

	if (cfg.direct_video && /* is_menu() */ 0) pr_flags = 0; // automatic pixel repetition
	else if (vm->param.pr != 0) pr_flags = 0b01001000; // manual pixel repetition with 2x clock
	else pr_flags = 0b01000000; // manual pixel repetition

	uint8_t sync_invert = 0;
	if (vm->param.hpol == 0) sync_invert |= 1 << 5;
	if (vm->param.vpol == 0) sync_invert |= 1 << 6;

	if (last_sync_invert == sync_invert && last_pr_flags == pr_flags && last_vic_mode == vic_mode) return;

	// address, value
	uint8_t init_data[] = {
		0x17, (uint8_t)(0b00000010 | sync_invert),		// Aspect ratio 16:9 [1]=1, 4:3 [1]=0
		0x3B, pr_flags,
		0x3C, vic_mode,			// VIC
	};

	for (unsigned i = 0; i < sizeof(init_data); i += 2)
	{
		row(init_data[i], init_data[i + 1]);
	}

	last_pr_flags = pr_flags;
	last_sync_invert = sync_invert;
	last_vic_mode = vic_mode;
}

static void print_rows(unsigned from, unsigned to)
{
	printf("[");
	for (unsigned i = from; i < to; i++)
	{
		printf("%s[%u, %u]", (i == from) ? "" : ", ", rows[i][0], rows[i][1]);
	}
	printf("]");
}

/*
 * The mode vectors.  The first two are the modes itsalive ships (vmodes[0] and
 * vmodes[6], video.cpp:127 and :133, both pr = 0 and both reaching set_mode
 * with hpol = vpol = 0); the rest exist only to give hpol, vpol and pr more
 * surface to be wrong on.  The pr = 1 rows are vmodes[14]'s case
 * (video.cpp:141, the only preset upstream ships with pixel repetition).
 */
static const vmode_param_t modes[] = {
	{  4, 0, 0, 0 },
	{  1, 0, 0, 0 },
	{  4, 0, 1, 1 },
	{  4, 0, 0, 1 },
	{  4, 0, 1, 0 },
	{  4, 1, 0, 0 },
	{  4, 1, 1, 1 },
	{ 16, 1, 0, 1 },
	{ 31, 0, 1, 0 },
	{  0, 0, 0, 0 },
};
#define NMODES (sizeof(modes) / sizeof(modes[0]))

int main(void)
{
	cfg_defaults();

	nrows = 0;
	hdmi_config_init();

	printf("{\n");
	printf("  \"generator\": \"tests/golden/adv7513-gen.sh\",\n");
	printf("  \"source\": \"Main_MiSTer video.cpp @ 6cda9cc: hdmi_config_init :1462-1615, hdmi_config_audio :1417-1460, hdmi_config_set_csc :1180-1400, hdmi_config_set_mode :1680-1727\",\n");
	printf("  \"hdmi_has_int\": %d,\n", HDMI_HAS_INT);
	printf("  \"init_len\": %u,\n", audio_begin - init_begin);
	printf("  \"audio_len\": %u,\n", csc_begin - audio_begin);
	printf("  \"csc_len\": %u,\n", nrows - csc_begin);
	printf("  \"writes\": ");
	print_rows(0, nrows);
	printf(",\n");

	printf("  \"modes\": [\n");
	for (unsigned i = 0; i < NMODES; i++)
	{
		vmode_custom_t vm;
		vm.param = modes[i];

		hdmi_invalidate_mode_cache();
		unsigned begin = nrows;
		hdmi_config_set_mode(&vm);

		printf("    { \"vic\": %u, \"pr\": %u, \"hpol\": %u, \"vpol\": %u, \"rows\": ",
			vm.param.vic, vm.param.pr, vm.param.hpol, vm.param.vpol);
		print_rows(begin, nrows);
		printf(" }%s\n", (i + 1 == NMODES) ? "" : ",");
	}
	printf("  ],\n");

	/*
	 * How many rows a second identical call emits with the cache left alone:
	 * zero, because of the early return at video.cpp:1706.  itsalive has no
	 * cache and would emit three.  See adv7513.rs's mode_regs() doc comment.
	 */
	{
		vmode_custom_t vm;
		vm.param = modes[0];
		hdmi_invalidate_mode_cache();
		hdmi_config_set_mode(&vm);
		unsigned begin = nrows;
		hdmi_config_set_mode(&vm);
		printf("  \"repeat_rows_without_invalidate\": %u\n", nrows - begin);
	}

	printf("}\n");
	return 0;
}
