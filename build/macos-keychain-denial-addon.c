#include <CoreFoundation/CoreFoundation.h>
#include <Security/Security.h>
#include <stdint.h>

typedef struct napi_env__ *napi_env;
typedef struct napi_value__ *napi_value;
typedef int32_t napi_status;
extern napi_status napi_create_int32(napi_env env, int32_t value, napi_value *result);

__attribute__((visibility("default"))) napi_value napi_register_module_v1(napi_env env,
                                                                          napi_value exports) {
  (void)exports;
  const void *keys[] = {kSecClass, kSecAttrService, kSecAttrAccount, kSecUseAuthenticationUI,
                        kSecReturnData};
  const void *values[] = {kSecClassGenericPassword, CFSTR("com.talkingquill.app.keyboard-owner"),
                          CFSTR("owner-ipc-v1"), kSecUseAuthenticationUIFail, kCFBooleanTrue};
  CFDictionaryRef query = CFDictionaryCreate(kCFAllocatorDefault, keys, values, 5,
                                              &kCFTypeDictionaryKeyCallBacks,
                                              &kCFTypeDictionaryValueCallBacks);
  OSStatus status = errSecAllocate;
  if (query != NULL) {
    CFTypeRef result = NULL;
    status = SecItemCopyMatching(query, &result);
    if (result != NULL) CFRelease(result);
    CFRelease(query);
  }
  napi_value output = NULL;
  if (napi_create_int32(env, status, &output) != 0) return NULL;
  return output;
}
