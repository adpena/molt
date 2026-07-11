#include <emmintrin.h>
#include <stddef.h>

#if defined(_WIN32)
#define MOLT_EXPORT __declspec(dllexport)
#else
#define MOLT_EXPORT __attribute__((visibility("default")))
#endif

MOLT_EXPORT void molt_correlate1d_scalar(
    const double *input,
    size_t output_length,
    const double *weights,
    size_t radius,
    float *output
) {
    for (size_t pixel = 0; pixel < output_length; ++pixel) {
        double value = input[pixel + radius] * weights[radius];
        for (size_t tap = 0; tap < radius; ++tap) {
            value += (input[pixel + tap] + input[pixel + 2 * radius - tap]) * weights[tap];
        }
        output[pixel] = (float)value;
    }
}

MOLT_EXPORT void molt_correlate1d_sse2(
    const double *input,
    size_t output_length,
    const double *weights,
    size_t radius,
    float *output
) {
    size_t pixel = 0;
    for (; pixel + 1 < output_length; pixel += 2) {
        __m128d value = _mm_mul_pd(
            _mm_loadu_pd(input + pixel + radius),
            _mm_set1_pd(weights[radius])
        );
        for (size_t tap = 0; tap < radius; ++tap) {
            __m128d left = _mm_loadu_pd(input + pixel + tap);
            __m128d right = _mm_loadu_pd(input + pixel + 2 * radius - tap);
            __m128d pair = _mm_add_pd(left, right);
            value = _mm_add_pd(value, _mm_mul_pd(pair, _mm_set1_pd(weights[tap])));
        }
        __m128 packed = _mm_cvtpd_ps(value);
        _mm_storel_pi((__m64 *)(output + pixel), packed);
    }
    if (pixel < output_length) {
        molt_correlate1d_scalar(input + pixel, 1, weights, radius, output + pixel);
    }
}

MOLT_EXPORT void molt_correlate1d_scalar_rows(
    const double *input,
    size_t row_count,
    size_t input_stride,
    size_t output_length,
    const double *weights,
    size_t radius,
    float *output
) {
    for (size_t row = 0; row < row_count; ++row) {
        molt_correlate1d_scalar(
            input + row * input_stride,
            output_length,
            weights,
            radius,
            output + row * output_length
        );
    }
}

MOLT_EXPORT void molt_correlate1d_sse2_rows(
    const double *input,
    size_t row_count,
    size_t input_stride,
    size_t output_length,
    const double *weights,
    size_t radius,
    float *output
) {
    for (size_t row = 0; row < row_count; ++row) {
        molt_correlate1d_sse2(
            input + row * input_stride,
            output_length,
            weights,
            radius,
            output + row * output_length
        );
    }
}
