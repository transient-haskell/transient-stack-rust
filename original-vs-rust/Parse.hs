{-# LANGUAGE ScopedTypeVariables, OverloadedStrings #-}
import Transient.Internals
import Transient.Parse
import Control.Monad
import Control.Applicative
import Control.Exception hiding (try)
import Control.Monad.IO.Class
import qualified Data.ByteString.Lazy.Char8 as BS
import Data.Char
import Data.IORef

-- runs a parser on a string: the result and what is left unparsed
p :: Show a => String -> TransIO a -> String -> IO ()
p label parser input = do
  r <- (do
    (res, _) <- runTransient $ withParseString (BS.pack input) $ do
                  x <- parser
                  rest <- notParsed
                  return (x, BS.unpack rest)
    evaluate (length (show res)) >> return (show res)) `catch` \(e :: SomeException) -> return ("EXCEPCION " ++ takeWhile (/= '\n') (show e))
  putStrLn (label ++ "  " ++ show input ++ " => " ++ r)

-- the parser over a stream of chunks
ps :: Show a => String -> TransIO a -> [String] -> IO ()
ps label parser chunks = do
  ref <- newIORef chunks
  let more = liftIO $ atomicModifyIORef ref $ \cs -> case cs of
                [] -> ([], SDone)
                [c] -> ([], SLast (BS.pack c))
                (c:r) -> (r, SMore (BS.pack c))
  (res, _) <- runTransient $ withParseStream more $ do
                x <- parser
                b <- getParseBuffer
                return (x, BS.unpack b)
  putStrLn (label ++ "  " ++ show chunks ++ " => " ++ show res)

main = do
  putStrLn "-- numbers"
  p "int" int "12abc"; p "int" int "-5"; p "int" int "+7"; p "int" int " 3"; p "int" int ""; p "int" int "abc"; p "int" int "007"
  p "integer" integer "123456789012345678901234567890x"; p "integer" integer "-9"
  p "hex" hex "ff"; p "hex" hex "FFz"; p "hex" hex "g"; p "hex" hex "0x1"; p "hex" hex "1A2b rest"
  mapM_ (p "double" double) ["3.5", "-3.5", "-0.5", "1e2", "1.5e2", "2E1", "7", "1e-2", "0.25x", "3.", ".5", "abc"]
  putStrLn "-- atomicity"
  p "string" (string "ab") "abc"; p "string" (string "ab") "ac"; p "string" (string "ab") "a"
  p "string|string" (string "ab" <|> string "ac") "acd"
  p "tChar" (tChar 'a') "abc"; p "tChar" (tChar 'a') "xbc"; p "anyChar" anyChar "xyz"; p "anyChar" anyChar ""
  p "composite without try" ((tChar 'a' >> tChar 'b') <|> tChar 'a') "ac"
  p "composite with try" (try (tChar 'a' >> tChar 'b') <|> tChar 'a') "ac"
  putStrLn "-- take / skip"
  p "tTakeWhile" (tTakeWhile isDigit) "123abc"; p "tTakeWhile empty" (tTakeWhile isDigit) "abc"
  p "tTakeWhile'" (tTakeWhile' isDigit) "123;abc"; p "tTake" (tTake 2) "abcd"; p "tTake" (tTake 9) "ab"; p "tDrop" (tDrop 2 >> notParsed) "abcd"
  p "tTakeUntil" (tTakeUntil (\s -> BS.isPrefixOf "::" s)) "ab::cd"; p "tTakeUntil no end" (tTakeUntil (\s -> BS.isPrefixOf "::" s)) "abcd"
  p "tDropUntil" (tDropUntil (\s -> BS.isPrefixOf "::" s) >> notParsed) "ab::cd"
  p "tTakeUntilToken" (tTakeUntilToken "::") "ab::cd"; p "tTakeUntilToken no token" (tTakeUntilToken "::") "abcd"
  p "tDropUntilToken" (tDropUntilToken "::" >> notParsed) "ab::cd"
  p "dropSpaces" (dropSpaces >> notParsed) "  \t\n x"; p "dropTillEndOfLine" (dropTillEndOfLine >> notParsed) "ab\ncd"
  p "parseString" parseString "  hello world"; p "parseString empty" parseString "   "
  p "tPutStr" (tPutStr "XY" >> notParsed) "ab"
  p "isDone" (isDone) ""; p "isDone not" (isDone) "a"
  putStrLn "-- combinators"
  p "manyTill" (manyTill anyChar (tChar ';')) "abc;def"; p "manyTill empty" (manyTill anyChar (tChar ';')) ";x"; p "manyTill no end" (manyTill anyChar (tChar ';')) "abc"
  p "sepBy" (sepBy int (tChar ',')) "1,2,3x"; p "sepBy empty" (sepBy int (tChar ',')) "x"; p "sepBy tail" (sepBy int (tChar ',')) "1,2,"
  p "commaSep" (commaSep int) "4,5"; p "semiSep" (semiSep int) "4;5"
  p "parens" (parens int) "(12)x"; p "parens malformed" (parens int) "(12x"; p "braces" (braces int) "{1}"; p "brackets" (brackets int) "[1]"; p "angles" (angles int) "<1>"
  p "between" (between (tChar '[') (tChar ']') int) "[7]"
  p "applicative" ((,) <$> int <*> (tChar ',' *> int)) "3,4rest"
  p "applicative fails" ((,) <$> int <*> (tChar ',' *> int)) "3;4"
  p "chainMany" (chainMany (:) (tChar 'a')) "aaab"
  putStrLn "-- streams by chunks"
  ps "int split" int ["12", "34", "ab"]; ps "string split" (string "abc") ["a", "b", "cd"]
  ps "tTakeWhile split" (tTakeWhile isDigit) ["1", "23", "4x", "5"]; ps "one byte per chunk" (do a <- int; tChar ','; b <- int; return (a, b)) ["1","2",",","3","4"]
  ps "tTakeUntilToken split" (tTakeUntilToken "::") ["ab:", ":cd"]
  ps "SLast" int ["12"]; ps "empty stream" (isDone) []
  ps "double split" double ["3", ".", "25"]
  putStrLn "-- withParseString restores"
  (r, _) <- runTransient $ do
     setParseString "outside"
     x <- withParseString "inside" (tTake 3)
     y <- tTake 2
     return (x, y)
  print r
